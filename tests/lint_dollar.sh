#!/bin/bash
# 发出去的 bash 里，变量名后面不能紧跟非 ASCII 字符：macOS 的 bash 3.2 在 UTF-8 下会把字的头一个字节吞进变量名，变量名要加花括号。
# 这里是要查的文件的名单：cca、install.sh、release.sh、Team 的同步服务器的安装脚本（服务器上的 sh 可能是 bash）、macos 和 ios 下的脚本、
# App 里拼出来的 bash（Swift 源码，比如终端交接脚本）、Claude Code 的 hook（.claude/hooks）；是目录的查它下面的 .sh、.swift。tests/ 下的不在名单里。
#   bash tests/lint_dollar.sh cca-test的路径 仓库的根 cca的路径
# 打出 cca-test lint dollar 的那一行：ok，或者「：文件:行号、…」（见 tests/tool/lint.rs）。回归第 11 节的 lint_dollar、.claude/hooks/quick-lint.sh 都用这一份名单
exec "$1" lint dollar "$2" "$3" install.sh release.sh server/hub-install.sh 'macos/*.sh' 'ios/*.sh' macos/Sources macos/tools '.claude/hooks/*.sh'
