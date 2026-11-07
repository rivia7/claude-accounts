#!/bin/bash
# cca App 的契约测试：编译 CcaKit + Tests/ContractTests/*.swift（本机架构）并运行。
#
#   bash macos/test.sh          手写的 Tests/Fixtures：通用不变量 + 针对这些数据的断言 + 各部件的单元测试
#   bash macos/test.sh 目录      真实的 cca 输出（tests/regress.sh 10g 生成）：只解码 + 通用不变量，缺的文件跳过
#
# 另外检查 CcaKit 没有 import SwiftUI / AppKit。有失败时退出码非 0。只写 macos/build/ 下面的文件。
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
BUILD="$HERE/build"

die() { printf '✗ %s\n' "$*" >&2; exit 1; }

if [ $# -gt 1 ]; then
	die "用法：bash macos/test.sh [真实输出的目录]"
elif [ $# -eq 1 ]; then
	[ -d "$1" ] || die "没有这个目录：$1"
	MODE=--generic
	DIR=$(cd "$1" && pwd)
else
	MODE=--full
	DIR="$HERE/Tests/Fixtures"
fi

# CcaKit 只能用 Foundation / Observation / CoreServices（界面相关的都在 CcaApp）
bad=$(grep -rlE '^[[:space:]]*(@testable[[:space:]]+)?import[[:space:]]+(SwiftUI|AppKit|Cocoa|UIKit)([^[:alnum:]_]|$)' \
	"$HERE/Sources/CcaKit" || true)
[ -z "$bad" ] || die "CcaKit 里不能 import SwiftUI / AppKit：$bad"
printf '✓ CcaKit 没有 import SwiftUI / AppKit\n' >&2

command -v swiftc >/dev/null 2>&1 || die "没有 swiftc（先装 Command Line Tools：xcode-select --install）"

SOURCES=()
while IFS= read -r f; do SOURCES+=("$f"); done < <(find "$HERE/Sources/CcaKit" "$HERE/Tests/ContractTests" -name '*.swift' | LC_ALL=C sort)
[ ${#SOURCES[@]} -gt 1 ] || die "没找到 Sources/CcaKit、Tests/ContractTests 下的源文件"

ARCH=$(uname -m)
BIN="$BUILD/contract-tests"
mkdir -p "$BUILD/tmp"
printf '• 编译契约测试（%s）…\n' "$ARCH" >&2
swiftc -Onone -swift-version 5 -module-name cca -target "$ARCH-apple-macos26.0" "${SOURCES[@]}" -o "$BIN"

CCA_TEST_TMP="$BUILD/tmp" TMPDIR="$BUILD/tmp/" "$BIN" "$MODE" "$DIR"
