#!/usr/bin/env python3
"""客户端统一更新：bash 那一半（cmd_clients）——安装要一字不差的令牌、有效的计划、用户的同意。

辅助程序那一半（client-updates：status、check、mode、auto-install、plan、install，签名、目标漂移、中断恢复……）原来
也在这份文件里（把内嵌的 Python 当库执行，70 个用例）。现在在辅助程序自己的测试里：`cli/tools/gen_clients_scenarios.py`
拿原来的 Python 把那 70 个用例一步一步走了一遍，每一步的输出、退出码、起的命令、写的文件记在
`cli/tests/vectors/src_clients.json`，`cmd::clients::tests::like_python`（cargo test）照着重放、逐步核对。"""
from pathlib import Path
import subprocess
import tempfile
import unittest


CCA = Path(__file__).resolve().parents[1] / 'cca'
SHELL = CCA.read_text()


def cmd_clients():
    """cca 里的 cmd_clients 函数（原文）"""
    start = SHELL.index('\ncmd_clients() {') + 1
    return SHELL[start:SHELL.index('\n}', start) + 2]


class ClientUpdatesShell(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='cca-client-update-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()

    def test_shell_requires_exact_token_authorization_and_valid_plan(self):
        function = cmd_clients()
        token = 'a' * 64
        prologue = '''
die() { echo "$*" >&2; exit 1; }
fail_warn() { :; }
client_update_py() {
  echo "$1 $2" >> "$TEST_LOG"
  if [ "$1" = plan ]; then
    [ "$PLAN_OK" = 1 ] || return 1
    echo "CU_VERSION=2.0; CU_INSTALLED=1.0; CU_TARGET=/tmp/ChatGPT.app; CU_LABEL=ChatGPT"
  else echo '{}'; fi
}
consent() { echo "consent $1 $2" >> "$TEST_LOG"; return "$CONSENT_RC"; }
'''
        for supplied, plan, consent, installs in [(token, '1', '1', False),
                                                   (token, '0', '0', False),
                                                   ('not-a-token', '1', '0', False),
                                                   (token, '1', '0', True)]:
            log = self.root / 'shell.log'
            log.write_text('')
            env = {'PATH': '/usr/bin:/bin', 'TEST_LOG': str(log), 'PLAN_OK': plan, 'CONSENT_RC': consent}
            result = subprocess.run(['/bin/bash', '-c', prologue + function + '\ncmd_clients install "$1" --json',
                                     'test', supplied], env=env, capture_output=True, text=True)
            lines = log.read_text().splitlines()
            self.assertEqual('install ' + token in lines, installs, result.stderr + str(lines))
            if plan == '0' or supplied != token:
                self.assertFalse(any(x.startswith('consent ') for x in lines))
            if installs:
                self.assertIn('consent client_update ' + token, lines)

    def test_consent_says_how_a_terminal_tool_is_updated(self):
        function = cmd_clients()
        token = 'a' * 64
        prologue = '''
die() { echo "$*" >&2; exit 1; }
fail_warn() { :; }
client_update_py() {
  if [ "$1" = plan ]; then printf '%s\\n' "CU_VERSION=2.1; CU_INSTALLED=2.0; CU_TARGET=/x/claude; CU_LABEL=Claude; CU_NOTE=${NOTE@Q}"
  else echo '{}'; fi
}
consent() { printf '%s\\n' "$5" >> "$TEST_LOG"; return 0; }
'''
        prologue = prologue.replace('${NOTE@Q}', "\"'$NOTE'\"")
        for note, expected in [('用 claude update 更新', '用 claude update 更新'), ('', '目标：/x/claude')]:
            log = self.root / 'shell.log'
            log.write_text('')
            result = subprocess.run(['/bin/bash', '-c', prologue + function + '\ncmd_clients install "$1" --json', 'test', token],
                                    env={'PATH': '/usr/bin:/bin', 'TEST_LOG': str(log), 'NOTE': note}, capture_output=True, text=True)
            self.assertIn(expected, log.read_text(), result.stderr)


if __name__ == '__main__':
    unittest.main()
