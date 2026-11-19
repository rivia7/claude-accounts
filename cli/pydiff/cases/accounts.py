"""env 等账号相关的命令（cli/src/cmd/accounts.rs），以及它用到的 codex 探测（cli/src/cmd/codex.rs 的 codex_probe）。"""
from run import case
from cases.table import world, TABLE, NODES

PORTS = {'主号(alice@example.com)': 7901, 'bob@example.com': 7902, '走节点': 7904}

# 假的 codex：认 features list / --help / --version，把每次收到的参数和要紧的环境记进沙箱里的 codex.log
FAKE_CODEX = r'''
printf 'ARGS: %s | CODEX_HOME=%s | OPENAI=%s | cwd=%s\n' "$*" "$CODEX_HOME" "${OPENAI_API_KEY-unset}" "$PWD" >> "$HOME/codex.log"
case "$1 $2" in
	"features list") [ -f "$HOME/codex_noflags" ] && { echo "other_feature  stable  true"; exit 0; }
		[ -f "$HOME/codex_fail" ] && exit 2
		printf 'system_proxy_fallback  experimental  true\nrespect_system_proxy stable true\n\nsomething_else  x\n' ;;
	"--help ") [ -f "$HOME/codex_nodaemon" ] && echo "Usage: codex [OPTIONS]" || echo "  --no-daemon   do not attach" ;;
	"--version ") echo "codex-cli 0.167.2" ;;
esac
'''

for key in ['主号(alice@example.com)', 'bob@example.com', '直连', '走节点', '旧名(alice@example.com)', '没有这个', '']:
    @case('env/claude/%s' % (key or 'empty'), ['env', key])
    def _(b):
        world(b, cfg={'ports': PORTS, 'no_proxy': 'localhost, .local,::1,10.0.0.0/8,,'})


@case('env/claude/full-api', ['env', '全'])
def _(b):
    world(b, table={'全': {'base_url': 'https://api.example.com/v1//', 'model': 'big-model', 'node': 'US-A', 'id': 'quan',
                          'env': {'A': 1, 'B': 'x y', 'HTTPS_PROXY': 'no', '1bad': 'no', 'C': 1.5, 'D': True}}})


@case('env/claude/no-add-dir', ['env', 'bob@example.com'])
def _(b):
    world(b, cfg={'claude_add_dir': False})


@case('env/claude/no-proxy-host', ['env', 'bob@example.com'])
def _(b):
    world(b, cfg={'proxy_host': '', 'router': {}})


@case('env/claude/router-host-only', ['env', 'bob@example.com'])
def _(b):
    world(b, cfg={'proxy_host': '', 'router': {'host': '192.0.2.77'}})


@case('env/claude/self-path', ['env', '直连'], env={'CCA_SELF': "/opt/my tools/cca"})
def _(b):
    world(b)


@case('env/claude/ro', ['env', 'bob@example.com'], env={'CCA_RO': '1'})
def _(b):
    world(b)


@case('env/claude/bad-table', ['env', 'bob@example.com'])
def _(b):
    world(b, table={'bob@example.com': 'ZZ'})


CODEX_TABLE = {'工作(erin@example.com)': 'US-A', '接口': {'base_url': 'https://api.example.com/v1', 'model': 'gpt-x', 'node': 'HK-A'},
               '直连接口': {'base_url': 'https://api3.example.com/v1/'}}


def codex_world(b, **marks):
    world(b, codex=CODEX_TABLE)
    b.fake('codex', FAKE_CODEX)
    for m in marks:
        b.write(m, '')


for key in ['工作(erin@example.com)', '接口', '直连接口']:
    @case('env/codex/%s' % key, ['env', key], tool='codex', env={'CCA_BIN': '%HOME%/bin/codex', 'OPENAI_API_KEY': 'sk-leak', 'CODEX_HOME': '/elsewhere'})
    def _(b):
        codex_world(b)


@case('env/codex/no-bin', ['env', '接口'], tool='codex')
def _(b):
    codex_world(b)


@case('env/codex/missing-bin', ['env', '接口'], tool='codex', env={'CCA_BIN': '%HOME%/bin/nope'})
def _(b):
    codex_world(b)


@case('env/codex/no-flags', ['env', '接口'], tool='codex', env={'CCA_BIN': '%HOME%/bin/codex'})
def _(b):
    codex_world(b, codex_noflags=1, codex_nodaemon=1)


@case('env/codex/features-fail', ['env', '接口'], tool='codex', env={'CCA_BIN': '%HOME%/bin/codex'})
def _(b):
    codex_world(b, codex_fail=1)


@case('env/codex/cached-probe', ['env', '工作(erin@example.com)'], tool='codex', env={'CCA_BIN': '%HOME%/bin/codex'})
def _(b):
    codex_world(b)
    import os
    st = os.stat(b.path('bin/codex'))
    b.json('.cca/codex/probe.json', {'sig': '%s|%d|%d' % (os.path.realpath(b.path('bin/codex')), st.st_size, int(st.st_mtime)),
                                    'flags': ['--disable', 'only_this'], 'no_daemon': False, 'version': '9.9'})


@case('env/codex/stale-probe', ['env', '工作(erin@example.com)'], tool='codex', env={'CCA_BIN': '%HOME%/bin/codex'})
def _(b):
    codex_world(b)
    b.json('.cca/codex/probe.json', {'sig': 'old', 'flags': [], 'no_daemon': False, 'version': '0.1'})


@case('env/codex/npm-js', ['env', '工作(erin@example.com)'], tool='codex', env={'CCA_BIN': '%HOME%/pkg/bin/codex.js'})
def _(b):
    codex_world(b)
    b.write('pkg/bin/codex.js', '#!/bin/sh\n' + FAKE_CODEX, 0o755)
    b.json('pkg/package.json', {'version': '0.200.1'})


@case('env/agy', ['env', 'gina@example.com'], tool='agy')
def _(b):
    world(b, agy={'gina@example.com': 'US-B'})
