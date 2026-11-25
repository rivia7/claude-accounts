#!/bin/bash
# 用法：swiftterm-parser.sh 上游的EscapeSequenceParser.swift 输出
# 写出去掉了图形协议（Kitty、sixel）的一份 EscapeSequenceParser.swift：Mac App（macos/build.sh）和 iOS App（ios/cca.xcodeproj
# 编 SwiftTerm 的那一步）共用。为什么、换版本时怎么办见 macos/Vendor/SwiftTerm/README.md「不要的功能」。
# 两处都得正好各有一处、都换上了才写出去：上游改了写法就停在这里（退出码 1），不能悄悄带着它编出去
set -eu
src=$1 out=$2
kitty='case 0x47: terminal.handleKittyGraphics(content)'
sixel='return SixelDcsHandler(terminal: terminal)'
[ -f "$src" ] || { echo "没找到 $src" >&2; exit 1; }
if [ "$(grep -cF "$kitty" "$src" || true)" != 1 ] || [ "$(grep -cF "$sixel" "$src" || true)" != 1 ]; then
	echo "SwiftTerm 的 EscapeSequenceParser.swift 里分派图形协议的那两行对不上（上游改了写法？）：见 macos/Vendor/SwiftTerm/README.md「不要的功能」" >&2
	exit 1
fi
tmp="$out.tmp.$$"
sed -e "s|$kitty|case 0x47: break|" -e "s|$sixel|return nil|" "$src" >"$tmp"
if grep -qF -e "$kitty" -e "$sixel" "$tmp" || ! grep -qF 'case 0x47: break' "$tmp"; then
	rm -f "$tmp"
	echo "没能把 SwiftTerm 分派图形协议的那两行换掉" >&2
	exit 1
fi
mv "$tmp" "$out"
