#!/bin/bash
# 编译 cca.app。只要 Command Line Tools（swiftc、lipo、sips、iconutil、codesign），不要 Xcode。
#
#   bash macos/build.sh [--version V] [--out PATH.app] [--arch universal|arm64|native] [--debug]
#
#   --version V   App 的版本号（默认取 ../cca 里 VERSION="…" 那一行；必须和 cca 的版本一样，App 靠它判断要不要更新）
#   --out PATH    输出的 .app（默认 macos/build/cca.app）
#   --arch A      universal（默认，arm64 + x86_64）| arm64 | native（本机）
#   --debug       不优化（-Onone -g），只编本机架构；带上 DEBUG 才有的截图模式（Snapshot.swift）
#
# 步骤：编译 CcaKit + CcaApp → 画图标（有缓存）→ 写 Info.plist → 组装 → ad-hoc 签名 → 校验签名 → 运行 --self-check。
# stdout 只输出 .app 的路径，进度写 stderr。
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/.." && pwd)
BUILD="$HERE/build"

VERSION=""
OUT="$BUILD/cca.app"
ARCH=universal
DEBUG=0

info() { printf '• %s\n' "$*" >&2; }
ok() { printf '✓ %s\n' "$*" >&2; }
die() { printf '✗ %s\n' "$*" >&2; exit 1; }

usage() {
	sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//' >&2
	exit "${1:-0}"
}

while [ $# -gt 0 ]; do
	case "$1" in
		--version) [ $# -ge 2 ] || die "--version 后面要跟版本号"; VERSION=$2; shift 2 ;;
		--version=*) VERSION=${1#--version=}; shift ;;
		--out) [ $# -ge 2 ] || die "--out 后面要跟 .app 的路径"; OUT=$2; shift 2 ;;
		--out=*) OUT=${1#--out=}; shift ;;
		--arch) [ $# -ge 2 ] || die "--arch 后面要跟 universal / arm64 / native"; ARCH=$2; shift 2 ;;
		--arch=*) ARCH=${1#--arch=}; shift ;;
		--debug) DEBUG=1; shift ;;
		-h|--help) usage 0 ;;
		*) printf '✗ 不认识的参数：%s\n' "$1" >&2; usage 2 ;;
	esac
done

if [ -z "$VERSION" ]; then
	[ -f "$ROOT/cca" ] || die "找不到 $ROOT/cca：用 --version 指定版本号"
	VERSION=$(sed -n 's/^VERSION="\([^"]*\)".*/\1/p' "$ROOT/cca" | head -n 1)
	[ -n "$VERSION" ] || die "读不到 $ROOT/cca 里的 VERSION：用 --version 指定"
fi
[[ "$VERSION" =~ ^[0-9A-Za-z][0-9A-Za-z.+-]*$ ]] || die "版本号只能有字母、数字和 .+-：$VERSION"

case "$OUT" in
	*.app) ;;
	*) die "--out 要以 .app 结尾：$OUT" ;;
esac
case "$OUT" in
	/*) ;;
	*) OUT="$PWD/$OUT" ;;
esac

HOST=$(uname -m)
if [ "$DEBUG" = 1 ]; then
	ARCH=native
fi
case "$ARCH" in
	universal) ARCHS="arm64 x86_64" ;;
	arm64) ARCHS="arm64" ;;
	native) ARCHS="$HOST" ;;
	*) die "--arch 只能是 universal / arm64 / native：$ARCH" ;;
esac

for tool in swiftc lipo sips iconutil codesign plutil; do
	command -v "$tool" >/dev/null 2>&1 || die "没有 ${tool}（先装 Command Line Tools：xcode-select --install）"
done

SOURCES=()
while IFS= read -r f; do SOURCES+=("$f"); done < <(find "$HERE/Sources/CcaKit" "$HERE/Sources/CcaApp" -name '*.swift' | LC_ALL=C sort)
[ ${#SOURCES[@]} -gt 0 ] || die "没找到源文件"

OBJ="$BUILD/obj"
mkdir -p "$OBJ" "$BUILD/icon"
if [ "$DEBUG" = 1 ]; then
	OPT=(-Onone -g -D DEBUG)
else
	OPT=(-O)
fi

# ---------------------------------------------------------------- 编译（几个架构同时编）
info "编译 cca ${VERSION}（$ARCHS$( [ "$DEBUG" = 1 ] && printf '，调试版' )）…"
pids=""
for a in $ARCHS; do
	(
		swiftc -parse-as-library "${OPT[@]}" -swift-version 5 -module-name cca -target "$a-apple-macos26.0" \
			"${SOURCES[@]}" -o "$OBJ/cca-$a" >"$OBJ/build-$a.log" 2>&1
	) &
	pids="$pids $!"
done
failed=0
for p in $pids; do
	wait "$p" || failed=1
done
if [ "$failed" = 1 ]; then
	for a in $ARCHS; do
		if grep -q 'error:' "$OBJ/build-$a.log" 2>/dev/null; then
			printf '\n----- %s -----\n' "$a" >&2
			grep -E 'error:' -A3 "$OBJ/build-$a.log" | head -n 60 >&2
		fi
	done
	die "编译失败（完整日志在 $OBJ/build-*.log）"
fi
SLICES=()
for a in $ARCHS; do SLICES+=("$OBJ/cca-$a"); done
if [ ${#SLICES[@]} -gt 1 ]; then
	BIN="$OBJ/cca-universal"
	lipo -create "${SLICES[@]}" -output "$BIN"
else
	BIN="${SLICES[0]}"
fi
ok "编译好了：$(lipo -archs "$BIN")"

# ---------------------------------------------------------------- 图标（make-icon 和 Logo 的字形没改过就用缓存）
ICNS="$BUILD/icon/AppIcon.icns"
# 字形 LogoGlyph.swift 和 App 里的 BrandMark 共用，一起编进 make-icon
ICON_SRC=("$HERE/tools/make-icon/main.swift" "$HERE/Sources/CcaApp/LogoGlyph.swift")
if [ ! -s "$ICNS" ] || [ "${ICON_SRC[0]}" -nt "$ICNS" ] || [ "${ICON_SRC[1]}" -nt "$ICNS" ]; then
	info "画图标…"
	swiftc -O -swift-version 5 "${ICON_SRC[@]}" -o "$BUILD/icon/make-icon" >"$BUILD/icon/build.log" 2>&1 ||
		{ cat "$BUILD/icon/build.log" >&2; die "编译 make-icon 失败"; }
	"$BUILD/icon/make-icon" "$BUILD/icon/icon_1024.png"
	SET="$BUILD/icon/AppIcon.iconset"
	rm -rf "$SET"
	mkdir -p "$SET"
	for s in 16 32 128 256 512; do
		sips -z "$s" "$s" "$BUILD/icon/icon_1024.png" --out "$SET/icon_${s}x${s}.png" >/dev/null
		d=$((s * 2))
		sips -z "$d" "$d" "$BUILD/icon/icon_1024.png" --out "$SET/icon_${s}x${s}@2x.png" >/dev/null
	done
	iconutil -c icns "$SET" -o "$BUILD/icon/AppIcon.tmp.icns"
	mv "$BUILD/icon/AppIcon.tmp.icns" "$ICNS"
	ok "图标：$ICNS"
fi

# ---------------------------------------------------------------- 组装
STAGE="$OUT.tmp.$$"
rm -rf "$STAGE"
trap 'rm -rf "$STAGE"' EXIT
mkdir -p "$STAGE/Contents/MacOS" "$STAGE/Contents/Resources/zh-Hans.lproj"
cp "$BIN" "$STAGE/Contents/MacOS/cca"
chmod 755 "$STAGE/Contents/MacOS/cca"
cp "$ICNS" "$STAGE/Contents/Resources/AppIcon.icns"
printf 'APPL????' >"$STAGE/Contents/PkgInfo"
# 只有中文：系统提供的菜单（退出、隐藏……）也显示中文
printf '"CFBundleDisplayName" = "cca";\n"CFBundleName" = "cca";\n' >"$STAGE/Contents/Resources/zh-Hans.lproj/InfoPlist.strings"
cat >"$STAGE/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>zh-Hans</string>
	<key>CFBundleLocalizations</key>
	<array>
		<string>zh-Hans</string>
	</array>
	<key>CFBundleExecutable</key>
	<string>cca</string>
	<key>CFBundleIdentifier</key>
	<string>local.cca.app</string>
	<key>CCAClientUpdateVerifier</key>
	<integer>1</integer>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>cca</string>
	<key>CFBundleDisplayName</key>
	<string>cca</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>$VERSION</string>
	<key>CFBundleVersion</key>
	<string>$VERSION</string>
	<key>CFBundleIconFile</key>
	<string>AppIcon</string>
	<key>LSMinimumSystemVersion</key>
	<string>26.0</string>
	<key>LSApplicationCategoryType</key>
	<string>public.app-category.developer-tools</string>
	<key>NSHighResolutionCapable</key>
	<true/>
	<key>NSPrincipalClass</key>
	<string>NSApplication</string>
	<key>NSSupportsAutomaticTermination</key>
	<false/>
	<key>NSSupportsSuddenTermination</key>
	<false/>
	<key>NSHumanReadableCopyright</key>
	<string>cca — https://github.com/rivia7/claude-accounts</string>
</dict>
</plist>
PLIST
plutil -lint "$STAGE/Contents/Info.plist" >/dev/null || die "Info.plist 不对"

# ---------------------------------------------------------------- 签名、校验、自检
codesign --force --sign - --timestamp=none "$STAGE" >/dev/null 2>&1 || die "codesign 失败"
codesign --verify --strict "$STAGE" || die "签名校验不过"
rm -rf "$OUT"
mkdir -p "$(dirname "$OUT")"
mv "$STAGE" "$OUT"
trap - EXIT
codesign --verify --strict "$OUT" || die "签名校验不过：$OUT"
ok "签好名了（ad-hoc）"

check=$("$OUT/Contents/MacOS/cca" --self-check 2>&1) || die "自检失败：$check"
[ "$check" = "ok $VERSION" ] || die "自检输出不对：${check}（应为 ok ${VERSION}）"
ok "自检：$check"
case " $ARCHS " in
	*" x86_64 "*)
		if [ "$HOST" != x86_64 ] && arch -x86_64 /usr/bin/true >/dev/null 2>&1; then
			check=$(arch -x86_64 "$OUT/Contents/MacOS/cca" --self-check 2>&1) || die "x86_64 自检失败：$check"
			[ "$check" = "ok $VERSION" ] || die "x86_64 自检输出不对：$check"
			ok "x86_64 自检（Rosetta）：$check"
		fi
		;;
esac

printf '%s\n' "$OUT"
