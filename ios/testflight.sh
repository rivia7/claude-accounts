#!/bin/bash
# iOS App 传到 TestFlight（App Store Connect）。要 Xcode，「设置 → Accounts」里登录着付费的开发者账号（免费的个人团队传不了）。
# 见 docs/dev/ios.md「TestFlight」。
#   bash ios/testflight.sh [--team 团队ID]            先跑 test.sh --build 的检查，再归档、签名、导出 .ipa 并查它的签名（ios/build/testflight/），不上传
#   bash ios/testflight.sh [--team 团队ID] --upload   同上，查过以后再导出一次、直接传到 App Store Connect（那里要先有套装 ID 是 com.cca.remote 的 App）
# 团队没写时用 Xcode 里登录着的付费团队（只有一个时）。
# 经 SSH 在别的 Mac 上跑时先解开登录钥匙串（security unlock-keychain），不然签名时报 errSecInternalComponent。
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="$HERE/build/testflight"
DD="$OUT/dd"
BUNDLE=com.cca.remote
fail() { echo "✗ $*" >&2; exit 1; }
ok() { echo "✓ $*"; }

TEAM="" UPLOAD=""
while [ $# -gt 0 ]; do
	case "$1" in
		--team) [ $# -ge 2 ] && [ -n "$2" ] || fail "--team 后面写团队 ID（Xcode「设置 → Accounts」里看得到）"; TEAM="$2"; shift 2 ;;
		--upload) UPLOAD=1; shift ;;
		*) fail "用法：bash ios/testflight.sh [--team 团队ID] [--upload]" ;;
	esac
done
command -v xcodebuild >/dev/null && xcodebuild -version >/dev/null 2>&1 || fail "要 Xcode（只有命令行工具编不了 iOS App）"

# 团队：Xcode 里登录着的付费团队。defaults 打出来的每个团队的键按字母排：isFreeProvisioningTeam 在 teamID 前面
if [ -z "$TEAM" ]; then
	# 参数前面都处理完了，位置参数拿来放找到的团队（团队 ID 只有字母和数字）；defaults 读不到就是一个也没有
	set -- $(defaults read com.apple.dt.Xcode IDEProvisioningTeamByIdentifier 2>/dev/null |
		awk '/isFreeProvisioningTeam =/ { free = $3 } /teamID =/ { id = $3; gsub(/[";]/, "", id); if (free == "0;") print id }' | sort -u)
	case $# in
		0) fail "Xcode 里没有登录着的付费开发者团队（「设置 → Accounts」；免费的个人团队传不了 TestFlight）" ;;
		1) TEAM=$1 ;;
		*) fail "Xcode 里有几个付费团队，用 --team 选一个：$*" ;;
	esac
fi
# 构建号：UTC 时间 年月日.时分秒，每次都比上次大（同一个版本号下 App Store Connect 不收重复的构建号）；时分秒去掉开头的 0，每一段都是整数
NOW=$(date -u +%Y%m%d.%H%M%S)
BUILD="${NOW%%.*}.$((10#${NOW#*.}))"

# 先跑 test.sh --build 的检查（查什么见那里）
bash "$HERE/test.sh" --build

rm -rf "$OUT/cca.xcarchive"
mkdir -p "$OUT"
xcodebuild -project "$HERE/cca.xcodeproj" -scheme cca -configuration Release -destination 'generic/platform=iOS' \
	-archivePath "$OUT/cca.xcarchive" -derivedDataPath "$DD" -allowProvisioningUpdates \
	DEVELOPMENT_TEAM="$TEAM" CURRENT_PROJECT_VERSION="$BUILD" archive >"$OUT/archive.log" 2>&1 ||
	{ grep -E "error:|errSec" "$OUT/archive.log" | sort -u | head -20 >&2; fail "没归档成（全部输出在 ${OUT}/archive.log）"; }
APP="$OUT/cca.xcarchive/Products/Applications/cca.app"
[ -d "$APP" ] || fail "归档里没有 cca.app"
plist() { plutil -extract "$2" raw "$1"; }   # 文件 键
[ "$(plist "$APP/Info.plist" CFBundleIdentifier)" = "$BUNDLE" ] || fail "套装 ID 不是 ${BUNDLE}"
[ "$(plist "$APP/Info.plist" CFBundleVersion)" = "$BUILD" ] || fail "构建号没换上（${BUILD}）"
VERSION=$(plist "$APP/Info.plist" CFBundleShortVersionString)
# 归档是另编的：test.sh 查正式版的那五项（SwiftTerm 编的是换过的那一份、没有 TestDriver、关了 ATS、通知扩展、提示音）在它上面再查一遍
bash "$HERE/test.sh" --check "$DD" "$APP"
ok "归档好了：${VERSION}（${BUILD}），团队 ${TEAM}"

export_as() {   # export|upload：照 App Store 的分发签名（Xcode 管的证书、描述文件），导出到 $OUT/export 或者直接上传（Xcode 留下的在 $OUT/upload）
	cat >"$OUT/ExportOptions.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>method</key><string>app-store-connect</string>
	<key>destination</key><string>$1</string>
	<key>teamID</key><string>$TEAM</string>
	<key>signingStyle</key><string>automatic</string>
	<key>uploadSymbols</key><true/>
	<key>manageAppVersionAndBuildNumber</key><false/>
</dict>
</plist>
EOF
	rm -rf "$OUT/$1"
	xcodebuild -exportArchive -archivePath "$OUT/cca.xcarchive" -exportOptionsPlist "$OUT/ExportOptions.plist" \
		-exportPath "$OUT/$1" -allowProvisioningUpdates >"$OUT/$1.log" 2>&1 ||
		{ grep -iE "error|errSec" "$OUT/$1.log" | sort -u | head -20 >&2; fail "没导出成（${1}；全部输出在 ${OUT}/${1}.log）"; }
}

# 先导出成 .ipa，查它的签名：Apple Distribution 签的、App Store 的描述文件（不带设备列表）、不能被调试器接上
export_as export
IPA=$(find "$OUT/export" -maxdepth 1 -name "*.ipa" | head -1)
[ -n "$IPA" ] || fail "没导出 .ipa"
X="$OUT/ipa"   # 解开的 .ipa、读出来的描述文件和权限，留着备查
rm -rf "$X"
ditto -x -k "$IPA" "$X"
A="$X/Payload/cca.app"
codesign --verify --deep --strict "$A" || fail "导出的 App 签名验不过"
SIGNER=$(codesign -dvv "$A" 2>&1) || fail "读不了导出的 App 的签名"
case "$SIGNER" in *"Authority=Apple Distribution"*) ;; *) fail "导出的 App 不是 Apple Distribution 签的" ;; esac
security cms -D -i "$A/embedded.mobileprovision" >"$X/profile.plist" 2>/dev/null || fail "读不了导出的 App 的描述文件"
if plist "$X/profile.plist" ProvisionedDevices >/dev/null 2>&1; then fail "描述文件带着设备列表：不是 App Store 的"; fi
codesign -d --entitlements - --xml "$A" >"$X/ent.plist" 2>/dev/null || fail "读不了导出的 App 的权限"
[ "$(plist "$X/ent.plist" get-task-allow 2>/dev/null || echo false)" = false ] || fail "导出的 App 允许调试器接上（get-task-allow）"
# 推送：App Store 的签名把 aps-environment 换成了 production；App 的钥匙串分组第一个是它自己的（身份、配对的密钥照旧在那里），
# 第二个和通知扩展共用；通知扩展只有共用的那一个，和两边 Info.plist 的 CcaKeychainGroup 一样（docs/dev/ios.md「推送」）
[ "$(plist "$X/ent.plist" aps-environment 2>/dev/null)" = production ] || fail "导出的 App 没有 aps-environment = production（推送收不到）"
SHARED="${TEAM}.com.cca.remote.shared"
[ "$(plist "$X/ent.plist" keychain-access-groups.0 2>/dev/null)" = "${TEAM}.${BUNDLE}" ] &&
	[ "$(plist "$X/ent.plist" keychain-access-groups.1 2>/dev/null)" = "$SHARED" ] || fail "导出的 App 的钥匙串分组不对（$X/ent.plist）"
N="$A/PlugIns/notify.appex"
codesign -d --entitlements - --xml "$N" >"$X/notify-ent.plist" 2>/dev/null || fail "读不了通知扩展的权限"
[ "$(plist "$X/notify-ent.plist" keychain-access-groups.0 2>/dev/null)" = "$SHARED" ] &&
	! plist "$X/notify-ent.plist" keychain-access-groups.1 >/dev/null 2>&1 || fail "通知扩展的钥匙串分组不对（$X/notify-ent.plist）"
[ "$(plist "$A/Info.plist" CcaKeychainGroup)" = "$SHARED" ] && [ "$(plist "$N/Info.plist" CcaKeychainGroup)" = "$SHARED" ] ||
	fail "Info.plist 里的 CcaKeychainGroup 不是 ${SHARED}（团队 ID 的前缀 AppIdentifierPrefix 没换上？）"
ok "导出、查过签名：${IPA}"
if [ -z "$UPLOAD" ]; then
	echo "没上传（要传到 TestFlight 加 --upload）"
	exit 0
fi

export_as upload
ok "传到 App Store Connect 了：${VERSION}（${BUILD}）。处理完（一般十几分钟到半小时）在 TestFlight 里回答出口合规，内部测试组就能装"
