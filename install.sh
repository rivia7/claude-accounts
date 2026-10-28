#!/bin/bash
# cca 一键安装：
#
#   curl -fsSL https://github.com/rivia7/claude-accounts/releases/latest/download/install.sh | bash
#
# 下载最新版 cca，校验发布签名，然后运行 cca install（会在终端里问几个问题：路由器、可选节点、第一个账号）。
# 已经装过的，再运行一次就是修复/重装，账号和配置都保留。
#
# https://github.com/rivia7/claude-accounts · SPDX-License-Identifier: MIT

REPO="https://github.com/rivia7/claude-accounts"
RELEASE_KEY="ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICxClUySZa2aVx7s6HRzufgJ49ad44smTnmkzDI/Osex cca-release"

# 整个安装过程包在函数里，脚本最后一行才执行：curl | bash 下载到一半断了也不会执行半截脚本
main() {
	local c py tmp f i v
	fail() { printf '\033[31m✗\033[0m %s\n' "$*" >&2; exit 1; }
	info() { printf '\033[32m•\033[0m %s\n' "$*" >&2; }

	[ "$(uname)" = Darwin ] || fail "cca 目前只支持 macOS"
	for c in curl ssh ssh-keygen perl; do
		command -v "$c" >/dev/null 2>&1 || fail "缺少命令 ${c}"
	done
	# macOS 的 /usr/bin/python3 只是个占位，没装命令行工具时一运行就弹安装窗口
	py=$(command -v python3 2>/dev/null)
	if [ -z "$py" ] || { [ "$py" = /usr/bin/python3 ] && ! xcode-select -p >/dev/null 2>&1; }; then
		fail "需要 python3：先运行 xcode-select --install 装上苹果的命令行工具，装好后再执行这条安装命令"
	fi
	command -v claude >/dev/null 2>&1 || [ -x "$HOME/.local/bin/claude" ] ||
		fail "没找到 Claude Code：先运行 curl -fsSL https://claude.ai/install.sh | bash 安装它，再执行这条安装命令"
	# curl | bash 时标准输入是管道，安装要回答问题，得从终端读
	if [ -z "${CCA_TTY:-}" ] && ! (: </dev/tty) 2>/dev/null; then
		fail "需要在终端里运行这条命令（安装过程要回答几个问题）"
	fi

	tmp=$(mktemp -d "${TMPDIR:-/tmp}/cca-install.XXXXXX") || fail "建不了临时目录"
	trap 'rm -rf "$tmp"' EXIT
	info "正在下载最新版 cca…"
	for f in cca cca.sig; do
		for i in 1 2 3; do   # 有的网络偶尔会丢新连接，多试几次
			curl -fsSL -m 120 -o "$tmp/$f" "$REPO/releases/latest/download/$f" 2>/dev/null && break
			[ "$i" = 3 ] && fail "下载失败（网络不通，或 GitHub 暂时不可用），稍后再试"
			sleep 2
		done
	done
	printf 'cca-release %s\n' "$RELEASE_KEY" > "$tmp/allowed"
	ssh-keygen -Y verify -f "$tmp/allowed" -I cca-release -n cca-release -s "$tmp/cca.sig" < "$tmp/cca" >/dev/null 2>&1 ||
		fail "签名校验没通过（文件可能被篡改或下载不完整），没有安装"
	v=$(sed -n 's/^VERSION="\([0-9.]*\)"$/\1/p' "$tmp/cca" | head -n 1)
	info "已下载 cca ${v}，签名校验通过"

	if [ -n "${CCA_TTY:-}" ]; then
		bash "$tmp/cca" install
	else
		bash "$tmp/cca" install </dev/tty
	fi
}

main "$@"
