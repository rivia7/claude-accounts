#!/bin/bash
# 全流程回归：端口池（槽位 + 策略组，API 切换不重载）、重载需同意、账号表、出口检测、登录校验、第三方 API 账号、
# 订阅用量、自动更新、还原
# 用法：bash tests/regress.sh（只在 macOS 上跑；全部用假命令，不联网、不碰真实的 ~/.cca 和路由器）
D=$(cd "$(dirname "$0")" && pwd)
CCA="$D/../cca"; CCA=$(cd "$(dirname "$CCA")" && pwd)/cca
T=$(mktemp -d "${TMPDIR:-/tmp}/cca-test.XXXXXX") && T=$(cd "$T" && pwd -P); H=$T/home; R=$T/router   # 规整成真实路径（TMPDIR 结尾有 / 时会出现 //）
trap '[ -n "${KEEP:-}" ] && echo "沙箱保留在 $T" || rm -rf "$T"' EXIT
bash "$D/setup.sh" "$T" >/dev/null || exit 1
cd "$T" || exit 1
run() { env -i LANG=en_US.UTF-8 HOME="$H" CCA_T="$T" CCA_CLAUDE_APP="$T/Claude.app" PATH="$T/bin:$H/.local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" TERM=dumb "$@"; }
rr() { env CCA_ROOT="$R" CCA_T="$T" CCA_WAIT=4 HOME="$R/root" PATH="$T/rbin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" /bin/dash "$R/etc/cca/cca-router.sh" "$@"; }
ans() { f="$T/ans.$RANDOM$RANDOM"; printf '%s\n' "$@" > "$f"; echo "$f"; }
pass=0; fail=0
ok() { if eval "$2"; then echo "PASS  $1"; pass=$((pass + 1)); else echo "FAIL  $1   [$2]"; fail=$((fail + 1)); fi; }
edit_table() { python3 -c "import json,sys; p=sys.argv[1]; j=json.load(open(p)); $1; json.dump(j,open(p,'w'),indent=2,ensure_ascii=False)" "$H/.cca/accounts.json"; }
RC="$R/etc/openclash/custom.yaml"
nl_() { ruby -ryaml -e 'c=YAML.load_file(ARGV[0]); print((c["listeners"]||[]).count { |l| l["name"].start_with?("cca-") })' "$RC"; }
first() { ruby -ryaml -e 'c=YAML.load_file(ARGV[0]); g=(c["proxy-groups"]||[]).find { |x| x["name"]==ARGV[1] }; print(g ? g["proxies"][0] : "-")' "$RC" "$1"; }
sel() { cat "$R/sel/$1" 2>/dev/null || echo -; }
reloads() { grep -c '^RELOAD' router.log 2>/dev/null || echo 0; }

# 1. 安装 + 第一个账号：建端口池需要一次重载，先征求同意
out=$(run CCA_TTY="$(ans y 192.0.2.10 '' '' '1 2 3 4' y alice@example.com '' 1 y)" bash "$CCA" install 2>&1)
ok "install：从路由器的节点里选可选节点" 'python3 -c "import json,sys; sys.exit(0 if json.load(open(sys.argv[1]))[\"nodes\"]==[\"US-A\",\"US-B\",\"US-C\",\"US-D\"] else 1)" "$H/.cca/config.json" && printf "%s" "$out" | grep -q "账号可选的出口节点：US-A、US-B、US-C、US-D"'
ok "install：建端口池前先问，提示重载约 20 秒" 'printf "%s" "$out" | grep -q "约 20 秒，期间局域网的新连接会中断"'
ok "install：8 个槽位 7901-7908，CCA-1 默认 US-A，空槽位默认 REJECT" '[ "$(nl_)" = 8 ] && [ "$(first CCA-1)" = US-A ] && [ "$(first CCA-8)" = REJECT ] && [ "$(sel CCA-1)" = US-A ]'
ok "install：只重载了一次" '[ "$(reloads)" = 1 ]'
ok "install：钩子 / cron / sysupgrade" 'grep -q ">>> cca >>>" "$R/etc/openclash/custom/openclash_custom_overwrite.sh" && grep -q ensure "$R/etc/crontabs/root" && grep -qx /etc/cca/ "$R/etc/sysupgrade.conf"'

# 2. 再加账号（邮箱、别名分两行填）：不重载，API 切换
out=$(run CCA_TTY="$(ans bob@example.com 工作 2)" cca add 2>&1)
ok "add bob：不重载，CCA-2 经 API 切到 US-B，文件里默认顺序同步更新" '[ "$(reloads)" = 1 ] && [ "$(sel CCA-2)" = US-B ] && [ "$(first CCA-2)" = US-B ]'
ok "add：邮箱和别名分两行，账号表的键是 别名(邮箱)" 'printf "%s" "$out" | grep -q "登录邮箱：" && printf "%s" "$out" | grep -q "别名（可选，回车跳过）：" && grep -q "\"工作(bob@example.com)\": \"US-B\"" "$H/.cca/accounts.json" && printf "%s" "$out" | grep -q "已添加 工作（bob@example.com），出口 US-B"'
out=$(run CCA_TTY="$(ans alice@example.com nope carol@example.com '工 作' 'a(b)' 工作 小号 3)" cca add 2>&1)
ok "add：重复邮箱、不像邮箱、别名带空格/括号/重名都会说明原因并重填" 'printf "%s" "$out" | grep -q "这个邮箱已经添加过了：alice@example.com" && printf "%s" "$out" | grep -q "这不像邮箱地址" && printf "%s" "$out" | grep -q "别名里不能有空格" && printf "%s" "$out" | grep -q "别名「工作」已经用在 工作（bob@example.com） 上了" && grep -q "\"小号(carol@example.com)\"" "$H/.cca/accounts.json"'
m=$(run CCA_TTY="$(ans q)" cca 2>&1)
ok "菜单：别名、邮箱分两列，没有别名的别名列留空" 'printf "%s" "$m" | grep -q "1\. *alice@example.com   未登录" && printf "%s" "$m" | grep -q "2\. 工作   bob@example.com     未登录"'
run CCA_TTY="$(ans)" cca web 小号 >/dev/null 2>&1
ok "web：没登录时先用账号邮箱登录（登录页预填），再打开 claude.ai" 'grep -q "ARGS: auth login --claudeai --email carol@example.com" claude.log && grep -q "https://claude.ai/" open.log'
run CCA_TTY="$(ans y)" cca rm 小号 >/dev/null 2>&1

# 3. 登录 + 启动 + 出口检测
run CCA_TTY="$(ans)" cca @alice --version >/dev/null 2>&1; rc=$?
ok "alice：预填邮箱登录 → 启动，退出码透传" '[ $rc -eq 3 ] && grep -q "ARGS: auth login --claudeai --email alice@example.com" claude.log'
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
rm -f "$T/away"; n0=$(vreloads); : > ssh.log
run CCA_TTY="$(ans)" cca @alice --version >/dev/null 2>&1; rc=$?
ok "回到家：自动走回路由器，补同步在外面改过的账号表（CCA-2 → US-B），不碰 Clash Verge" '[ $rc -eq 3 ] && grep -q "sh -s" ssh.log && [ "$(sel CCA-2)" = US-B ] && [ "$(vreloads)" = "$n0" ] && lastenv | grep -q "https_proxy=http://192.0.2.10:7901"'
run cca local off >/dev/null 2>&1
ok "cca local off：全局扩展脚本逐字节还原，端口池从内核里移除" 'cmp -s "$VS" "$T/pristine_script.js" && ! grep -qx 7901 "$T/verge/listening" && [ "$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))[\"local\"])" "$H/.cca/config.json")" = "{}" ]'
run CCA_TTY="$(ans y)" cca local setup >/dev/null 2>&1
ok "cca local setup：在家也能先配置好" 'grep -q ">>> cca >>>" "$VS" && grep -qx 7901 "$T/verge/listening"'
run CCA_TTY="$(ans y)" cca rm 小号 >/dev/null 2>&1

# 10c. 第三方 API 账号：直连 / 走节点；key 存钥匙串，经 apiKeyHelper 交给 Claude Code（不进命令行，也不进环境变量）
kcd() { ls -d "$T/keychain/cca-api-$1-"* 2>/dev/null | head -n 1; }   # 钥匙串里这个账号的条目（目录里一个服务商一个文件）
kc() { local d; d=$(kcd "$1"); [ -n "$d" ] && cat "$d"/* 2>/dev/null; }
kch() { local d; d=$(kcd "$1"); [ -n "$d" ] && ls "$d"; }   # 存它的服务商（接口的域名）
lastline() { grep "^$1: " claude.log | tail -n 1; }
S1=sk-test-AAAA1111; S2=sk-test-BBBB2222; S3=sk-test-CCCC3333
: > ssh.log; : > "$T/env.log"; rm -f "$T/security.stdin"
out=$(run CCA_TTY="$(ans kimi https://api.example.com/anthropic/ "$S1" kimi-k2 '')" cca add api 2>&1); rc=$?
ok "add api：表里是对象（地址去掉末尾的 /，带 id，没有 key），直连不占端口、不同步路由器" '[ $rc -eq 0 ] && python3 -c "import json,sys; e=json.load(open(sys.argv[1]))[\"kimi\"]; c=json.load(open(sys.argv[2])); sys.exit(0 if e==dict(base_url=\"https://api.example.com/anthropic\", model=\"kimi-k2\", node=\"\", id=\"kimi\") and \"kimi\" not in c[\"ports\"] else 1)" "$H/.cca/accounts.json" "$H/.cca/config.json" && ! grep -q "sh -s" ssh.log && printf "%s" "$out" | grep -q "已添加第三方 API 账号 kimi"'
ok "add api：key 经 security -i 的 stdin（十六进制）存进钥匙串，按接口的域名存，不出现在任何命令行里" '[ "$(kc kimi)" = "$S1" ] && [ "$(kch kimi)" = api.example.com ] && grep -q "add-generic-password -U -a api.example.com -s cca-api-kimi-" "$T/security.stdin" && ! grep -q "$S1" "$T/security.stdin" security.log "$T/env.log" ssh.log'
n_trace=$(grep -c "cdn-cgi/trace" curl.log); n_login=$(grep -c "ARGS: auth login" claude.log); : > "$T/env.log"; rm -f "$T/claude.key"
out=$(run CCA_TTY="$(ans)" ANTHROPIC_BASE_URL=http://evil.invalid ANTHROPIC_AUTH_TOKEN=sk-evil-0000 ANTHROPIC_MODEL=evil CLAUDE_CODE_USE_BEDROCK=1 CLAUDE_CODE_USE_MANTLE=1 CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST=1 https_proxy=http://192.0.2.99:3128 HTTPS_PROXY=http://192.0.2.99:3128 cca @kimi --version 2>&1); rc=$?
ok "API 账号启动：继承来的接口地址、凭证、模型、云厂商开关和代理都不带进去，并提示" '[ $rc -eq 3 ] && lastenv | grep -q "https_proxy=unset" && lastenv | grep -q "HTTPS_PROXY=unset" && lastenv | grep -q "ANTHROPIC_BASE_URL=unset" && lastenv | grep -q "ANTHROPIC_AUTH_TOKEN=unset" && lastenv | grep -q "ANTHROPIC_MODEL=unset" && lastenv | grep -q "CLAUDE_CODE_USE_BEDROCK=unset" && lastenv | grep -q "CLAUDE_CODE_USE_MANTLE=unset" && lastenv | grep -q "CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST=unset" && lastenv | grep -q "BROWSER=unset" && printf "%s" "$out" | grep -q "已为本次启动屏蔽环境里继承的"'
ok "API 账号启动：地址、模型、关遥测经 --settings 的 env 传；apiKeyHelper 回调 cca 取到钥匙串里的 key" 'lastline SETTINGS | grep -qF "\"ANTHROPIC_BASE_URL\": \"https://api.example.com/anthropic\"" && lastline SETTINGS | grep -qF "\"ANTHROPIC_MODEL\": \"kimi-k2\"" && lastline SETTINGS | grep -qF "\"ANTHROPIC_DEFAULT_HAIKU_MODEL\": \"kimi-k2\"" && lastline SETTINGS | grep -qF "\"CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC\": \"1\"" && [ "$(lastline HELPER)" = "HELPER: rc=0" ] && [ "$(cat "$T/claude.key")" = "$S1" ]'
ok "API 账号启动：直连检测接口，不做 claude.ai 出口检测，不走官方登录、不同步路由器；key 不出现在任何日志和命令行里" '! grep -q "sh -s" ssh.log && grep -q "CURL api direct https://api.example.com/anthropic" curl.log && [ "$(grep -c "cdn-cgi/trace" curl.log)" = "$n_trace" ] && [ "$(grep -c "ARGS: auth login" claude.log)" = "$n_login" ] && ! grep -q "$S1" claude.log "$T/env.log" security.log ssh.log curl.log open.log'
out=$(run CCA_TTY="$(ans)" ANTHROPIC_BASE_URL=http://evil.invalid cca @alice --version 2>&1); rc=$?
ok "官方账号：shell 里继承的 ANTHROPIC_BASE_URL 不带进去（否则登录 token 会发到别的地址），也不加 --settings" '[ $rc -eq 3 ] && lastenv | grep -q "ANTHROPIC_BASE_URL=unset" && lastenv | grep -q "BROWSER=$H/.cca/bin/open-url" && ! lastline ARGS | grep -q -- --settings && printf "%s" "$out" | grep -q "屏蔽环境里继承的 ANTHROPIC_BASE_URL"'
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
ok "cca usage：两个官方账号一起查，表里有套餐、5 小时和本周用量、分模型额度；注明第三方 API 账号不查" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -qE "alice@example.com +Max 5x +13%  2 小时 1[0-9] 分后重置 +41%  周. [0-9:]+ 重置" && printf "%s" "$out" | grep -qE "bob@example.com +Pro +98%" && printf "%s" "$out" | grep -q "Sonnet 本周 60%" && printf "%s" "$out" | grep -q "第三方 API 账号没有订阅用量，不查：kimi"'
ok "cca usage：每个账号经自己的端口、用自己的配置目录无界面查询（--safe-mode），查之前先看出口地区" 'grep -q "^ARGS: -p --safe-mode --no-session-persistence --strict-mcp-config --input-format stream-json --output-format stream-json --verbose$" claude.log && grep "^USAGE: " claude.log | tail -n 2 | grep -q "email=alice@example.com https_proxy=http://192.0.2.10:7901" && grep "^USAGE: " claude.log | tail -n 2 | grep -q "email=bob@example.com https_proxy=http://192.0.2.10:7902" && grep -q "CURL via :7902 https://claude.ai/cdn-cgi/trace" curl.log'
out=$(run cca usage --json 2>/dev/null)
ok "cca usage --json：stdout 只有 JSON（套餐、各窗口、数据来源、跳过的第三方 API 账号）" 'printf "%s" "$out" | python3 -c "import json,sys; d=json.load(sys.stdin); a={x[\"account\"]: x for x in d[\"accounts\"]}; sys.exit(0 if a[\"alice@example.com\"][\"five_hour\"][\"utilization\"]==13 and a[\"alice@example.com\"][\"plan\"]==\"Max 5x\" and a[\"bob@example.com\"][\"source\"]==\"live\" and a[\"bob@example.com\"][\"seven_day_sonnet\"][\"utilization\"]==60 and d[\"skipped_api_accounts\"]==[\"kimi\"] else 1)"'
ok "cca usage：查到的存进账号目录的 usage.json（菜单和查不到时用）" '[ -f "$AD/usage.json" ]'
m=$(run CCA_TTY="$(ans q)" cca 2>&1)
ok "菜单：官方账号附上最近一次的用量（读缓存，不联网）" 'printf "%s" "$m" | grep -q "5h 13% · 周 41%" && printf "%s" "$m" | grep -q "5h 98% · 周 72%"'
cp "$R/listening" "$T/listening.bak"; grep -vx 7902 "$T/listening.bak" > "$R/listening"; canned bob@example.com 50 50 pro
out=$(run cca usage 2>&1)
ok "出口端口不通：显示上次的缓存（注明多久前）和原因，不拿账号走别的路" 'printf "%s" "$out" | grep -qE "bob@example.com +Pro +98%" && printf "%s" "$out" | grep -qE "缓存（[0-9]+ 分钟前）：出口端口 7902 不通"'
cp "$T/listening.bak" "$R/listening"
touch "$T/usage/unsupported"
out=$(run cca usage alice 2>&1)
ok "Claude Code 不支持 get_usage：提示升级，用缓存顶上" 'printf "%s" "$out" | grep -q "这个版本的 Claude Code 查不了用量" && printf "%s" "$out" | grep -qE "alice@example.com +Max 5x +13%"'
rm -f "$T/usage/unsupported"; touch "$T/usage/hang"; t0=$(date +%s)
out=$(run CCA_USAGE_TIMEOUT=2 cca usage alice 2>&1); t1=$(date +%s)
ok "查询卡住：到时间就结束（不会一直等），显示缓存" 'printf "%s" "$out" | grep -q "查询超时" && [ $((t1 - t0)) -lt 15 ]'
rm -f "$T/usage/hang"; mkdir -p "$T/loc"; printf CN > "$T/loc/7901"; n_usage=$(grep -c "^USAGE: " claude.log)
out=$(run cca usage alice 2>&1)
ok "出口在受限地区：不拿账号去查，说明原因" 'printf "%s" "$out" | grep -q "出口在中国大陆" && [ "$(grep -c "^USAGE: " claude.log)" = "$n_usage" ]'
rm -f "$T/loc/7901"
out=$(run cca usage kimi 2>&1); rc=$?
ok "cca usage 第三方 API 账号：说明它没有订阅用量" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "没有订阅用量可查"'
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
r4() { env -i LANG=en_US.UTF-8 HOME="$H4" CCA_T="$T" PATH="$T/bin:$H4/.local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" TERM=dumb "$@"; }
r4 CCA_TTY="$(ans n n)" bash "$CCA" install >/dev/null 2>&1
out=$(r4 CCA_TTY="$(ans solo https://api.example.com/anthropic "$S1" '')" cca add api 2>&1); rc=$?
out2=$(r4 CCA_TTY="$(ans)" cca @solo --version 2>&1); rc2=$?
out3=$(r4 cca ls 2>&1)
ok "不配置路由器、只有直连的第三方 API 账号：照样能添加、启动和列出，不提路由器" '[ $rc -eq 0 ] && [ $rc2 -eq 3 ] && ! printf "%s%s%s" "$out" "$out2" "$out3" | grep -q "路由器" && printf "%s" "$out3" | grep -qE "solo +api.example.com +API +直连"'
r4 CCA_TTY="$(ans y)" cca rm solo >/dev/null 2>&1

# 11. 文字和编码
out=$(run cca router sync 2>&1); m=$(run CCA_TTY="$(ans q)" cca 2>&1)
ok "UTF-8 下中文提示完整" 'printf "%s" "$out" | grep -q "正在同步账号到路由器 192.0.2.10" && printf "%s" "$m" | grep -q "继续使用 alice@example.com" && printf "%s%s" "$out" "$m" | python3 -I -c "import sys; sys.stdin.buffer.read().decode(\"utf-8\")"'
ok "lint：cca 和 install.sh 里没有 \$变量 紧跟非 ASCII 字符" '[ "$(python3 -I -c "import re,sys; print(sum(len(re.findall(r\"\\\$[A-Za-z_][A-Za-z0-9_]*(?=[^\\x00-\\x7f])\", open(f, encoding=\"utf-8\").read())) for f in sys.argv[1:]))" "$CCA" "$D/../install.sh")" = 0 ]'
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
publish() {   # publish 版本 [路由器脚本标记] [签名密钥]
	mkdir -p "$G/v$1"; mkver "$1" "$G/v$1/cca" "${2:-}"; rm -f "$G/v$1/cca.sig"
	ssh-keygen -Y sign -q -n cca-release -f "${3:-$T/relkey}" "$G/v$1/cca" 2>/dev/null
	echo "$1" > "$G/latest"
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
ok "下次启动：自动换上 9.9.10，并用新版本执行这次命令" 'printf "%s" "$out" | grep -q "cca 已自动更新：2.0.0 → 9.9.10" && [ "$(ver)" = 9.9.10 ] && [ $rc -eq 3 ] && grep -q "^ARGS: --version" claude.log'
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

# 11c. 一键安装（install.sh）：全新的 HOME，下载最新版、验签、安装
H3="$T/home3"; mkdir -p "$H3"
sed "s|^RELEASE_KEY=\".*\"$|RELEASE_KEY=\"$TESTPUB test-release\"|" "$D/../install.sh" > "$T/install.sh"
r3() { env -i LANG=en_US.UTF-8 HOME="$H3" CCA_T="$T" SHELL=/bin/zsh PATH="$T/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" TERM=dumb "$@"; }
publish 9.9.20
out=$(r3 CCA_TTY="$(ans y n n)" bash "$T/install.sh" 2>&1); rc=$?
ok "一键安装：下载最新版、验签、装到 ~/.cca/bin 并链接" '[ $rc -eq 0 ] && [ "$("$H3/.cca/bin/cca" version)" = 9.9.20 ] && [ "$(readlink "$H3/.local/bin/cca")" = "$H3/.cca/bin/cca" ] && printf "%s" "$out" | grep -q "已下载 cca 9.9.20，签名校验通过"'
ok "一键安装：~/.local/bin 不在 PATH 里时，问过之后写进 ~/.zshrc" 'grep -qF "export PATH=\"\$HOME/.local/bin:\$PATH\"" "$H3/.zshrc" && printf "%s" "$out" | grep -q "打开一个新的终端标签，输入 cca 就能用"'
out=$(r3 CCA_TTY="$(ans n n)" bash "$T/install.sh" 2>&1); rc=$?
ok "再运行一次：修复/重装，不重复写 PATH" '[ $rc -eq 0 ] && [ "$(grep -c "\.local/bin" "$H3/.zshrc")" = 1 ] && printf "%s" "$out" | grep -q "已经写在"'
publish 9.9.21; echo "# 篡改" >> "$G/v9.9.21/cca"; rm -rf "$H3"; mkdir -p "$H3"
out=$(r3 CCA_TTY="$(ans y n n)" bash "$T/install.sh" 2>&1); rc=$?
ok "一键安装：签名不对就不装" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "签名校验没通过" && [ ! -e "$H3/.cca" ]'
n_gh=$(grep -c "GH latest-get" gh.log)
r3 bash -c "head -c 1500 '$T/install.sh' | bash" >/dev/null 2>&1
ok "install.sh 只下载了一半：什么都不执行" '[ "$(grep -c "GH latest-get" gh.log)" = "$n_gh" ] && [ ! -e "$H3/.cca" ]'

# 12. 一键还原：移除端口池需要重载（restore 已确认）
K=$(cat "$H/.cca/router_ed25519.pub"); echo "$K" >> "$R/root/.ssh/authorized_keys"
run CCA_TTY="$(ans restore)" cca restore >/dev/null 2>&1; rc=$?
ok "restore：端口池、钩子、cron、文件、公钥全部清掉，覆写脚本逐字节一致" '[ $rc -eq 0 ] && [ "$(nl_)" = 0 ] && ! ls "$R"/sel/CCA-* >/dev/null 2>&1 && cmp -s pristine_overwrite.sh "$R/etc/openclash/custom/openclash_custom_overwrite.sh" && [ ! -e "$R/etc/cca" ] && ! grep -q cca "$R/etc/crontabs/root" && [ "$(cat "$R/etc/sysupgrade.conf")" = "/root/.ssh/" ] && ! grep -qF "$K" "$R/root/.ssh/authorized_keys"'
ok "restore：启动器全部删掉" '[ -z "$(ls "$H/Applications" 2>/dev/null)" ]'
ok "restore：Clash Verge 全局扩展脚本里 cca 的那一段删掉（逐字节还原），本机端口池移除" 'cmp -s "$VS" "$T/pristine_script.js" && ! grep -qx 7901 "$T/verge/listening"'
ok "restore：本机还原，共享主目录内容保留" '[ ! -e "$H/.cca" ] && [ ! -L "$H/.local/bin/cca" ] && [ -f "$H/.claude/projects/-p1/memory/alice.md" ]'
ok "restore：第三方 API 账号的 key 从钥匙串删掉；配置备份只有自己能读（账号表里可能写着 key）" '[ -z "$(ls "$T/keychain" | grep "^cca-api-")" ] && [ "$(stat -f %Lp "$(ls "$H"/cca-backup-*.tar.gz | head -n 1)")" = 600 ]'
ok "所有重载前都用 OpenClash 的 SAFE_PATHS 做了 -t 校验" 'grep -q "CORE SAFE_PATHS=/usr/share/openclash:/etc/ssl -t" router.log'
echo "---- $pass passed, $fail failed"
