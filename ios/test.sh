#!/bin/bash
# iOS App：编、查、在模拟器里截图。要 Xcode 和 iOS 模拟器的运行时（xcodebuild -downloadPlatform iOS）。见 docs/dev/ios.md「测试」。
#   bash ios/test.sh               编（模拟器、调试版）、查 SwiftTerm 编进去的是去掉了图形协议的那一份、在模拟器里跑 SwiftTerm 的自检、
#                                  演示模式各页浅色 / 深色截图（ios/build/shots/）
#   bash ios/test.sh --build       只编（模拟器的调试版 + 真机的正式版，不签名），不开模拟器
#   bash ios/test.sh --check 派生数据目录 cca.app   不编，只查编好的正式版（SwiftTerm、TestDriver、ATS、通知扩展、提示音那五项）：testflight.sh 拿它查归档
# 端到端（真的服务器、真的 cca、模拟器里的 App 配对、接入、敲键）在 tests/ios_kit.rs 的 in_the_simulator：它先跑这里的编译那一步
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
OUT="$HERE/build"
DD="$OUT/dd"
DEVICE_NAME="cca-test iPhone"
BUNDLE=com.cca.remote
fail() { echo "✗ $*" >&2; exit 1; }
ok() { echo "✓ $*"; }

# 查正式版的五项，testflight.sh 拿它们（--check）再查一遍归档：
# SwiftTerm 编进去的是 swiftterm-parser.sh 换过那两行的那一份，不是上游原样的（macos/Vendor/SwiftTerm/README.md「不要的功能」）
swiftterm_patched() {   # 派生数据目录
	local lists list
	lists=$(find "$1/Build/Intermediates.noindex" -name SwiftTerm.SwiftFileList)
	[ -n "$lists" ] || fail "找不到 SwiftTerm 的编译清单（${1}）"
	for list in $lists; do
		grep -q "DerivedSources/EscapeSequenceParser.swift" "$list" || fail "SwiftTerm 没编换过的 EscapeSequenceParser.swift（${list}）"
		if grep -q "Vendor/SwiftTerm/Sources/SwiftTerm/EscapeSequenceParser.swift" "$list"; then fail "SwiftTerm 编了上游原样的 EscapeSequenceParser.swift（${list}）"; fi
	done
}
# 正式版里没有测试用的那一段（TestDriver）
no_test_driver() {   # cca.app
	if grep -rqa "cca-test: " "$1"; then fail "正式版里带着测试用的 TestDriver（${1}）"; fi
}
# Info.plist 关了 ATS（NSAllowsArbitraryLoads）：不然钉住的自签证书在 delegate 认了以后还被 ATS 照系统的规则拒掉，公网上的服务器
# 全都连不上（-1200 / -9802，App 说「连不上服务器」）。本机地址 ATS 不管：模拟器里连 127.0.0.1 的端到端测不出来，只能在这里查
ats_lets_us_pin() {   # cca.app
	[ "$(plutil -extract NSAppTransportSecurity.NSAllowsArbitraryLoads raw -o - "$1/Info.plist" 2>/dev/null)" = true ] ||
		fail "Info.plist 里没有 NSAppTransportSecurity.NSAllowsArbitraryLoads：钉住的自签证书会被 ATS 拒掉（${1}）"
}
# 通知扩展（ios/Notify）编进去了：推来的通知靠它解开（没有它时锁屏上只有推送里写的「远程会话里有新的通知」那一句）；版本号、构建号
# 和 App 的一样（App Store Connect 不收不一样的）；两边的 Info.plist 写着同一个共用的钥匙串分组（通知密钥在那里，docs/dev/ios.md「推送」）
notify_embedded() {   # cca.app
	local x="$1/PlugIns/notify.appex" k v
	[ -f "$x/Info.plist" ] || fail "App 里没有通知扩展（${x}）"
	[ "$(plutil -extract NSExtension.NSExtensionPointIdentifier raw -o - "$x/Info.plist" 2>/dev/null)" = com.apple.usernotifications.service ] ||
		fail "通知扩展的 NSExtensionPointIdentifier 不是 com.apple.usernotifications.service（${x}）"
	for k in CFBundleShortVersionString CFBundleVersion CcaKeychainGroup; do
		v=$(plutil -extract "$k" raw -o - "$1/Info.plist" 2>/dev/null) || fail "App 的 Info.plist 里没有 ${k}"
		[ "$(plutil -extract "$k" raw -o - "$x/Info.plist" 2>/dev/null)" = "$v" ] || fail "通知扩展的 ${k} 和 App 的不一样"
	done
	case "$v" in *com.cca.remote.shared) ;; *) fail "CcaKeychainGroup 不是 …com.cca.remote.shared：${v}" ;; esac
}
# 通知的两声提示音（ios/Sounds/，docs/dev/ios.md「App」的 Sounds/）：App 和通知扩展里各带着一份，和仓库里的一样
sounds_embedded() {   # cca.app
	local s
	for s in done.caf ask.caf; do
		cmp -s "$HERE/Sounds/$s" "$1/$s" || fail "App 里没有提示音 ${s}，或者和 ios/Sounds/ 里的不一样（${1}）"
		cmp -s "$HERE/Sounds/$s" "$1/PlugIns/notify.appex/$s" || fail "通知扩展里没有提示音 ${s}，或者和 ios/Sounds/ 里的不一样（${1}）"
	done
}
if [ "${1:-}" = --check ]; then swiftterm_patched "$2"; no_test_driver "$3"; ats_lets_us_pin "$3"; notify_embedded "$3"; sounds_embedded "$3"; exit 0; fi

command -v xcodebuild >/dev/null && xcodebuild -version >/dev/null 2>&1 || fail "要 Xcode（只有命令行工具编不了 iOS App）"
mkdir -p "$OUT"

build() {   # 配置 SDK 目标 日志
	xcodebuild -project "$HERE/cca.xcodeproj" -scheme cca -configuration "$1" -sdk "$2" -destination "$3" -derivedDataPath "$DD" \
		build "${@:5}" >"$4" 2>&1 || { grep -E "error:" "$4" | sort -u | head -20 >&2; fail "没编过（${1}，${2}；全部输出在 ${4}）"; }
	# cca 自己的代码不许有警告（SwiftTerm 是上游的，照它自己的写法）
	if grep -E "^/.*warning:" "$4" | grep -v "/Vendor/\|/macos/Vendor/" | sort -u | grep .; then fail "有警告（${1}，${2}）"; fi
}

build Debug iphonesimulator 'generic/platform=iOS Simulator' "$OUT/build-sim.log"
ok "编好了：模拟器的调试版"
build Release iphoneos 'generic/platform=iOS' "$OUT/build-device.log" CODE_SIGNING_ALLOWED=NO
ok "编好了：真机的正式版（没签名）"

swiftterm_patched "$DD"
grep -q "case 0x47: break" "$DD/Build/Intermediates.noindex/cca.build/Debug-iphonesimulator/SwiftTerm.build/DerivedSources/EscapeSequenceParser.swift" ||
	fail "换过的 EscapeSequenceParser.swift 不对"
ok "SwiftTerm 编的是去掉了图形协议的那一份"
# 调试版里得有 TestDriver，不然下一条查不出什么（它在 cca.debug.dylib 里，所以整个 .app 都查）
grep -rqa "cca-test: " "$DD/Build/Products/Debug-iphonesimulator/cca.app" || fail "调试版里没找到 TestDriver：这一条查不出什么了，改一改"
no_test_driver "$DD/Build/Products/Release-iphoneos/cca.app"
ok "正式版里没有测试用的 TestDriver"
ats_lets_us_pin "$DD/Build/Products/Debug-iphonesimulator/cca.app"
ats_lets_us_pin "$DD/Build/Products/Release-iphoneos/cca.app"
ok "两个版本的 Info.plist 都关了 ATS（钉住自签证书要它）"
notify_embedded "$DD/Build/Products/Debug-iphonesimulator/cca.app"
notify_embedded "$DD/Build/Products/Release-iphoneos/cca.app"
ok "两个版本里都有通知扩展，版本号、钥匙串分组和 App 的一样"
# 提示音就是 ios/tools/make-sounds 合成的那两声（改了声音忘了重新生成、手改了文件都查得出来）
xcrun swiftc -O "$HERE/tools/make-sounds/main.swift" -o "$OUT/make-sounds" >"$OUT/make-sounds.log" 2>&1 ||
	{ cat "$OUT/make-sounds.log" >&2; fail "make-sounds 没编过"; }
"$OUT/make-sounds" --check "$HERE/Sounds" >/dev/null || fail "ios/Sounds/ 里的提示音和 ios/tools/make-sounds 合成的不一样：改了声音以后用它重新生成"
sounds_embedded "$DD/Build/Products/Debug-iphonesimulator/cca.app"
sounds_embedded "$DD/Build/Products/Release-iphoneos/cca.app"
ok "提示音是 make-sounds 合成的那两声，两个版本的 App、通知扩展里都带着"
[ "${1:-}" = "--build" ] && exit 0

# ---- 模拟器 ----
# 不用 python3（仓库里不用 Python）：照 simctl 给人看的那种输出取，先后和 -j 的一样
UDID=$(xcrun simctl list devices available | sed -n "s/^ *${DEVICE_NAME} (\([0-9A-F-]\{36\}\)) .*/\1/p" | sed -n 1p)
if [ -z "$UDID" ]; then
	RT=$(xcrun simctl list runtimes available | sed -n 's/^iOS .* - \(com\.apple\.CoreSimulator\.SimRuntime\.iOS-[^ ]*\) *$/\1/p' | tail -1)
	TYPE=$(xcrun simctl list devicetypes | sed -n -e '/Max/d' -e 's/^iPhone[^(]*Pro[^(]* (\(com\.apple\.CoreSimulator\.SimDeviceType\.[^)]*\)) *$/\1/p' | tail -1)
	[ -n "$RT" ] && [ -n "$TYPE" ] || fail "找不到 iOS 模拟器的运行时或者 iPhone Pro 的机型（xcrun simctl list）"
	UDID=$(xcrun simctl create "$DEVICE_NAME" "$TYPE" "$RT")
fi
xcrun simctl bootstatus "$UDID" -b >/dev/null
ok "模拟器：${DEVICE_NAME}（${UDID}）"
echo "$UDID" >"$OUT/simulator"

# SwiftTerm 的自检（和 Mac App 同一份 macos/tools/swiftterm-check）：编成模拟器上的命令行程序，链 App 用的那一份 SwiftTerm，在模拟器里跑
SIMSDK=$(xcrun --sdk iphonesimulator --show-sdk-path)
PRODUCTS="$DD/Build/Products/Debug-iphonesimulator"
xcrun --sdk iphonesimulator swiftc -O -swift-version 5 -suppress-warnings -target arm64-apple-ios26.0-simulator -sdk "$SIMSDK" \
	-F "$PRODUCTS" -framework SwiftTerm "$ROOT/macos/tools/swiftterm-check/main.swift" -o "$OUT/swiftterm-check" >"$OUT/check-build.log" 2>&1 ||
	{ cat "$OUT/check-build.log" >&2; fail "SwiftTerm 的自检没编过"; }
xcrun simctl spawn "$UDID" "$OUT/swiftterm-check" >"$OUT/check.log" 2>&1 || { cat "$OUT/check.log" >&2; fail "SwiftTerm 的自检没过：图形协议没关掉"; }
ok "SwiftTerm 的自检（在模拟器里）：图形协议确实不处理"

# 截图：演示模式的几页，浅色、深色
APP="$PRODUCTS/cca.app"
xcrun simctl install "$UDID" "$APP"
SHOTS="$OUT/shots"
rm -rf "$SHOTS"
mkdir -p "$SHOTS"
shot() {   # 文件名 启动参数…（都在演示模式里）
	xcrun simctl terminate "$UDID" "$BUNDLE" >/dev/null 2>&1 || true
	xcrun simctl launch "$UDID" "$BUNDLE" -demo "${@:2}" >/dev/null
	sleep "$([ "$1" = terminal ] && echo 7 || echo 3)"
	xcrun simctl io "$UDID" screenshot "$SHOTS/$1-$mode.png" >/dev/null 2>&1
}
# 装好后头一次启动慢（3 秒时还在淡入）：先空跑一次
xcrun simctl launch "$UDID" "$BUNDLE" -demo >/dev/null
sleep 6
for mode in light dark; do
	xcrun simctl ui "$UDID" appearance "$mode"
	for screen in list accounts account stats cost account-cost sharing terminal pair settings launch sort tutorial whatsnew toast; do
		shot "$screen" -screen "$screen"
	done
	# 总览换到 ChatGPT、只看 Pro 套餐的样子；折合金额按天看
	shot accounts-codex -screen accounts -tool codex
	shot accounts-pro -screen accounts -plan Pro
	shot cost-days -screen cost -days
	# 新手教程的最后一步；账号页不带提示卡片（每周用量、折合金额两张图对齐的样子）
	shot tutorial-last -screen tutorial -step 4
	shot account-notips -screen account -tips off
	# 总览不带提示卡片（卡片上出口的问题、右上角的小标记、电脑报的问题）；出口有问题的账号（MacBook Air 上的「副号」）的那一页：
	# 「下一步」那句话、「网络出口」一节、「远程会话」开关关着时会话那一节的说法
	shot accounts-notips -screen accounts -tips off
	shot account-exit -screen account -account 5 -tips off
	# 总览换到 Antigravity（MacBook Air 一组最上面的「cca 有新版本」、路由器选错了节点的「Gemini」），那个账号的那一页
	shot accounts-agy -screen accounts -tool agy -tips off
	shot account-node -screen account -account 6 -tips off
done
xcrun simctl terminate "$UDID" "$BUNDLE" >/dev/null 2>&1 || true
xcrun simctl ui "$UDID" appearance light
ok "截图：$SHOTS"
