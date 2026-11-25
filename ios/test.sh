#!/bin/bash
# iOS App：编、查、在模拟器里截图。要 Xcode 和 iOS 模拟器的运行时（xcodebuild -downloadPlatform iOS）。见 docs/dev/ios.md「测试」。
#   bash ios/test.sh               编（模拟器、调试版）、查 SwiftTerm 编进去的是去掉了图形协议的那一份、在模拟器里跑 SwiftTerm 的自检、
#                                  演示模式各页浅色 / 深色截图（ios/build/shots/）
#   bash ios/test.sh --build       只编（模拟器的调试版 + 真机的正式版，不签名），不开模拟器
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

# SwiftTerm 编进去的是 swiftterm-parser.sh 换过那两行的那一份，不是上游原样的（macos/Vendor/SwiftTerm/README.md「不要的功能」）
for list in $(find "$DD/Build/Intermediates.noindex" -name SwiftTerm.SwiftFileList); do
	grep -q "DerivedSources/EscapeSequenceParser.swift" "$list" || fail "SwiftTerm 没编换过的 EscapeSequenceParser.swift（${list}）"
	if grep -q "Vendor/SwiftTerm/Sources/SwiftTerm/EscapeSequenceParser.swift" "$list"; then fail "SwiftTerm 编了上游原样的 EscapeSequenceParser.swift（${list}）"; fi
done
grep -q "case 0x47: break" "$DD/Build/Intermediates.noindex/cca.build/Debug-iphonesimulator/SwiftTerm.build/DerivedSources/EscapeSequenceParser.swift" ||
	fail "换过的 EscapeSequenceParser.swift 不对"
ok "SwiftTerm 编的是去掉了图形协议的那一份"
# 正式版里没有测试用的那一段（TestDriver）
# （调试版里它在 cca.debug.dylib 里，所以整个 .app 都查）
grep -rqa "cca-test: " "$DD/Build/Products/Debug-iphonesimulator/cca.app" || fail "调试版里没找到 TestDriver：这一条查不出什么了，改一改"
if grep -rqa "cca-test: " "$DD/Build/Products/Release-iphoneos/cca.app"; then fail "正式版里带着测试用的 TestDriver"; fi
ok "正式版里没有测试用的 TestDriver"
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
for mode in light dark; do
	xcrun simctl ui "$UDID" appearance "$mode"
	for screen in list terminal pair settings; do
		xcrun simctl terminate "$UDID" "$BUNDLE" >/dev/null 2>&1 || true
		xcrun simctl launch "$UDID" "$BUNDLE" -demo -screen "$screen" >/dev/null
		sleep "$([ "$screen" = terminal ] && echo 7 || echo 3)"
		xcrun simctl io "$UDID" screenshot "$SHOTS/$screen-$mode.png" >/dev/null 2>&1
	done
done
xcrun simctl terminate "$UDID" "$BUNDLE" >/dev/null 2>&1 || true
xcrun simctl ui "$UDID" appearance light
ok "截图：$SHOTS"
