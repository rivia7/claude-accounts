#!/bin/bash
# 全流程回归：端口池（槽位 + 策略组，API 切换不重载）、重载需同意、账号表、出口检测、登录校验、自动更新、还原
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
ok "启动器：每个账号一个，改别名后自动改名，图标用 Claude App 的" '[ -x "$L/Claude alice.app/Contents/MacOS/cca-launcher" ] && [ -d "$L/Claude 副号.app" ] && [ ! -e "$L/Claude 工作.app" ] && plutil -extract CFBundleIdentifier raw "$L/Claude 副号.app/Contents/Info.plist" | grep -q "^local.cca.launcher.bob-example-com-" && cmp -s "$T/Claude.app/Contents/Resources/electron.icns" "$L/Claude 副号.app/Contents/Resources/AppIcon.icns"'
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
ok "restore：本机还原，共享主目录内容保留" '[ ! -e "$H/.cca" ] && [ ! -L "$H/.local/bin/cca" ] && [ -f "$H/.claude/projects/-p1/memory/alice.md" ]'
ok "所有重载前都用 OpenClash 的 SAFE_PATHS 做了 -t 校验" 'grep -q "CORE SAFE_PATHS=/usr/share/openclash:/etc/ssl -t" router.log'
echo "---- $pass passed, $fail failed"
