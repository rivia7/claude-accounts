#!/usr/bin/env python3
"""邮箱助手（cca mail）：把 cca 的辅助程序（cli/ 的 cca-core）当黑盒，经本机的假 Gmail（tests/mail_fake.py：真的 TLS、只读 IMAP，
假代理同时认 SOCKS5 和 HTTP CONNECT）走一遍 setup / test / open / 登录时盯邮箱 / forget：只读的会话、登录和代理出的错、证书照常
校验、没有「所有邮件」时看收件箱、Gmail 的搜索跟不上时照样找到新邮件、断线重连不漏邮件、最新的一封只开一次、链接只经 stdin。

每个用例一个临时 HOME，只连 127.0.0.1；钥匙串是假的 security、系统代理是假的 scutil（都在 PATH 最前面），打开链接的
cca _mail-open 换成记下链接的小脚本。应用专用密码是编的（abcdefghijklmnop），照样只从 stdin 交给辅助程序。

原来这一份把内嵌的 Python 当库执行。不用连 Gmail 的那些检查（认登录邮件的规则、连 Gmail 的路、解析、核对、盯邮箱的假时钟、
打开过的不再打开）现在在辅助程序自己的单元测试里，照原来的输入和结论一条条搬过去的（cargo test，回归的 unit 一节也跑）；
各在哪里见下面每个类名后面的那一行。"""
import json
import os
from pathlib import Path
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time
import types
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import mail_fake as mf  # noqa: E402
from helper_bin import HELPER  # noqa: E402

# CCA_TEST_MAILFAKE=rust：假 Gmail 和假代理换成 Rust 的那一份（cca-test mail serve，tests/kit/mailfake.rs），两份对照用；
# cca-test 在 CCA_TEST_BIN，没有就是 cli/target/kit/cca-test（先 cd cli && cargo build --profile kit -p cca-tests）
RUST_FAKE = os.environ.get('CCA_TEST_MAILFAKE') == 'rust'
TESTBIN = os.environ.get('CCA_TEST_BIN') or str(Path(__file__).resolve().parents[1] / 'cli' / 'target' / 'kit' / 'cca-test')

PW = 'abcdefghijklmnop'
ALL = '"[Gmail]/All Mail"'
SPAM = '"[Gmail]/Spam"'
MAC = sys.platform == 'darwin'

# 原来的各个类现在在哪（辅助程序的单元测试，名字是 Rust 的测试函数）：
# Rules：cli/src/cmd/mail.rs 的 link_must_be_exactly_a_claude_login_link、sender_domain_by_label_suffix、subject
# Route：cli/src/cmd/mail/route.rs 的 scutil_parse_top_level_only、system_proxy_socks_first_then_https_then_http、system_proxy_exceptions、explicit_specs、never_through_an_account_exit
# Parse：cli/src/cmd/mail/imap/tests.rs 的 internaldate、search_criteria、fetch_parse_any_order
# Judge、Clean、Links：cli/src/cmd/mail/judge.rs 的 every_check_in_mail_view（每条检查照字面；judgement_like_python 另外和原来 Python 的结果逐项比）
# Used：cli/src/cmd/mail/hunt.rs 的 claim_once_across_processes、old_records_are_dropped
# Hunt：cli/src/cmd/mail/hunt.rs 里同名的 14 个（opens_only_the_newest_and_keeps_watching …… refused_counts_as_rejected）
# Source：cli/src/cmd/mail/imap/tests.rs 的 first_round_searches_and_takes_the_latest_burst、later_rounds_fetch_new_uids_without_searching、uidvalidity_change_searches_again、a_round_cut_short_loses_nothing
# Lint：test_every_rule_is_known 在 cli/src/cmd/mail.rs 的 every_rule_is_known；test_python38_syntax_and_lazy_imports 查的是内嵌 Python 的写法，没有了
# Keystore：下面的 Keystore（经假的 security 走真的进程）；原来进程里的那一条在 cli/src/cmd/secrets.rs 的 write_replaces_the_same_account_only_and_delete_can_pick_one

# 假的钥匙串（和 tests/setup.sh 的一样）：条目存在 ~/keychain/<服务名>/<账户名>，每次的命令行记在
# ~/security.log，-i 从 stdin 读到的命令记在 ~/security.stdin；~/keychain_locked 在的时候存不进去
SECURITY = r'''#!/bin/sh
T="$HOME"; KC="$T/keychain"
printf 'SECURITY %s\n' "$*" >> "$T/security.log"
one() {
	[ $# -gt 0 ] || return 1
	cmd="$1"; shift
	svc=""; acct=""; hex=""; w=0; prev=""
	for a in "$@"; do
		case "$prev" in -s) svc="$a" ;; -a) acct="$a" ;; -X) hex="$a" ;; esac
		[ "$a" = -w ] && w=1
		prev="$a"
	done
	[ -n "$svc" ] || return 1
	if [ -n "$acct" ]; then f="$KC/$svc/$acct"; else f=$(ls "$KC/$svc/"* 2>/dev/null | head -n 1); fi
	case "$cmd" in
		add-generic-password)
			[ -f "$T/keychain_locked" ] && return 36
			mkdir -p "$KC/$svc"
			printf '%s' "$hex" | xxd -r -p > "$KC/$svc/${acct:-_}"
			return 0 ;;
		find-generic-password)
			[ -n "$f" ] && [ -f "$f" ] || return 44
			[ $w = 1 ] && { cat "$f"; echo; }
			return 0 ;;
		delete-generic-password)
			[ -n "$f" ] && [ -f "$f" ] || return 44
			rm -f "$f"; rmdir "$KC/$svc" 2>/dev/null
			return 0 ;;
	esac
	return 1
}
if [ "${1:-}" = -i ]; then
	while IFS= read -r line; do
		printf '%s\n' "$line" >> "$T/security.stdin"
		set -f; set -- $line; set +f
		one "$@"
	done
	exit 0
fi
one "$@"
'''
# 假的 scutil：系统代理就是假代理。用例都写明了连接方式；哪一条不小心走了「照系统代理」，也连不到真的 Gmail 上去
SCUTIL = '#!/bin/sh\ncat "$HOME/scutil.txt"\n'
# 假的 cca：辅助程序起 `$CCA_SELF _mail-open <账号>`、链接从 stdin 给它。记下账号和链接；另记参数个数、环境变量里有没有链接
OPENER = '''#!/bin/sh
[ "$1" = _mail-open ] || exit 9
IFS= read -r url
printf "%%s %%s\\n" "$2" "$url" >> %(opened)s
printf "%%s %%s\\n" "$#" "$(env | grep -c magic-link)" >> %(seen)s
exit "$(cat %(rc)s 2>/dev/null || echo 0)"
'''
PKI = None


def setUpModule():
    global PKI
    if MAC:
        PKI = tempfile.mkdtemp(prefix='cca-mail-pki-')
        mf.make_pki(os.path.join(PKI, 'pki'))


def tearDownModule():
    if PKI:
        shutil.rmtree(PKI, ignore_errors=True)


class Running:
    """正在跑的一条辅助程序命令：输出接在文件上，跑着的时候就能看"""

    def __init__(self, t, argv, env):
        self.t = t
        n = len(t.running)
        self.out, self.err = (Path(t.temp.name) / ('run-%d.%s' % (n, x)) for x in ('out', 'err'))
        with open(self.out, 'wb') as o, open(self.err, 'wb') as e:
            self.p = subprocess.Popen([HELPER] + list(argv), stdin=subprocess.DEVNULL, stdout=o, stderr=e, env=env, cwd=str(t.home))
        t.running.append(self)

    def stderr(self):
        return self.err.read_text(encoding='utf-8', errors='replace')

    def wait(self, timeout=60):
        code = self.p.wait(timeout)
        return code, self.out.read_text(encoding='utf-8', errors='replace'), self.stderr()

    def kill(self):
        if self.p.poll() is None:
            self.p.kill()
            self.p.wait()


class Watch:
    """后台的 cca-core mail-watch（登录时 bash 起的那个）。中间隔一个 sh 当它的父进程，就像登录时的 cca：
    over() 是 cca 不在了（sh 退出，mail-watch 看父进程没了自己收尾、记下上次的结果）；term() 是登录结束时的 TERM"""

    def __init__(self, t, key, env):
        self.t = t
        n = len(t.running)
        self.err, pidf = (Path(t.temp.name) / ('watch-%d.%s' % (n, x)) for x in ('err', 'pid'))
        script = 'err=$1; pidf=$2; shift 2; "$@" 2>"$err" </dev/null & echo $! > "$pidf"; read _'
        self.sh = subprocess.Popen(['/bin/sh', '-c', script, 'sh', str(self.err), str(pidf), HELPER, 'mail-watch', key],
                                   stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, env=env, cwd=str(t.home))
        t.running.append(self)
        self.pid = None
        self.pid = int(t.wait_for(lambda: pidf.exists() and pidf.read_text().endswith('\n') and pidf.read_text(), 'mail-watch 起来'))

    def stderr(self):
        return self.err.read_text(encoding='utf-8', errors='replace') if self.err.exists() else ''

    def gone(self):
        try:
            os.kill(self.pid, 0)
        except OSError:
            return True
        return False

    def release(self):
        if self.sh.poll() is None:
            self.sh.stdin.close()
            self.sh.wait(20)

    def over(self):
        """cca 不在了：它自己收尾。返回它说的话"""
        self.release()
        self.t.wait_for(self.gone, 'mail-watch 看父进程不在了自己退出')
        return self.stderr()

    def term(self):
        """登录结束：TERM，马上走"""
        self.kill(signal.SIGTERM)
        self.t.wait_for(self.gone, 'mail-watch 收到 TERM 退出')
        return self.stderr()

    def kill(self, sig=signal.SIGKILL):
        if self.pid is not None and not self.gone():
            try:
                os.kill(self.pid, sig)
            except OSError:
                pass
        self.release()


@unittest.skipUnless(MAC, '邮箱助手只在 macOS 上有（别的系统的辅助程序不带 TLS、没有钥匙串）')
class Box(unittest.TestCase):
    """每个用例一个临时 HOME；每个子类一个假 Gmail 和假代理（真的 TLS：证书是 setUpModule 里一次性的 CA 签的，
    辅助程序经 CCA_MAIL_CAFILE 信任它——这个变量只在 HOME 换过、连的是本机时才算数）"""
    keystore = 'file'   # 密钥文件（~/.cca/secrets/cca-mail）；'keychain' 是假的 security

    @classmethod
    def setUpClass(cls):
        cls.dir = tempfile.mkdtemp(prefix='cca-mail-wire-')
        shutil.copytree(os.path.join(PKI, 'pki'), os.path.join(cls.dir, 'pki'))
        cls.ca = os.path.join(cls.dir, 'pki', 'ca.pem')
        for k, _, _ in mf.FOLDERS:
            os.makedirs(os.path.join(cls.dir, 'box', k), exist_ok=True)
        if RUST_FAKE:
            cls.fake = subprocess.Popen([TESTBIN, 'mail', 'serve', cls.dir], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL)
            ports, end = os.path.join(cls.dir, 'ports.json'), time.time() + 30
            while not os.path.exists(ports):
                if cls.fake.poll() is not None or time.time() > end:
                    raise RuntimeError('cca-test mail serve 没起来')
                time.sleep(0.02)
            with open(ports) as f:
                p = json.load(f)
            cls.srv, cls.px = types.SimpleNamespace(port=p['imap']), types.SimpleNamespace(port=p['proxy'])
            return
        cls.srv = mf.Server(cls.dir)
        cls.srv.start()
        cls.px = mf.Proxy(cls.srv)
        cls.px.start()

    @classmethod
    def tearDownClass(cls):
        if RUST_FAKE:
            cls.fake.terminate()
            cls.fake.wait(10)
        else:
            cls.srv.ls.close()
            cls.px.ls.close()
        shutil.rmtree(cls.dir, ignore_errors=True)

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='cca-mail-')
        self.addCleanup(self.temp.cleanup)
        top = Path(self.temp.name)
        self.home = top / 'home'
        (self.home / '.cca').mkdir(parents=True)
        (self.home / 'bin').mkdir()
        self.opened, self.rc, self.seen = top / 'opened.log', top / 'rc', top / 'seen.log'
        fake = top / 'fake-cca'
        for path, text in ((self.home / 'bin' / 'security', SECURITY), (self.home / 'bin' / 'scutil', SCUTIL),
                           (fake, OPENER % {'opened': self.opened, 'seen': self.seen, 'rc': self.rc})):
            path.write_text(text)
            path.chmod(0o755)
        (self.home / 'scutil.txt').write_text('<dictionary> {\n  SOCKSEnable : 1\n  SOCKSPort : %d\n  SOCKSProxy : 127.0.0.1\n}\n' % self.px.port)
        (self.home / '.cca' / 'accounts.json').write_text(json.dumps({'alice@example.com': 'US-A', '工作(bob@example.com)': 'US-B'}))
        self.env = {'HOME': str(self.home), 'CCA_HOME': str(self.home / '.cca'), 'CCA_TOOL': 'claude', 'CCA_SELF': str(fake),
                    'PATH': '%s:/usr/bin:/bin:/usr/sbin:/sbin' % (self.home / 'bin'), 'LANG': 'en_US.UTF-8',
                    'CCA_MAIL_CAFILE': self.ca, 'CCA_MAIL_POLL': '0.1', 'CCA_MAIL_RETRY': '0.1'}
        if self.keystore == 'file':
            self.env['CCA_KEYSTORE'] = 'file'
        self.assertEqual(shutil.which('security', path=self.env['PATH']), str(self.home / 'bin' / 'security'), '真的钥匙串不能碰')
        for k, _, _ in mf.FOLDERS:
            for f in Path(self.dir, 'box', k).glob('*.eml'):
                f.unlink()
        self.ctl({})
        open(os.path.join(self.dir, 'imap.log'), 'w').close()
        self.socks = 'socks5://127.0.0.1:%d' % self.px.port
        self.http = 'http://127.0.0.1:%d' % self.px.port
        self.secret = self.home / '.cca' / 'secrets' / 'cca-mail'
        self.running = []
        self.addCleanup(lambda: [r.kill() for r in self.running])

    # --- 假 Gmail
    def ctl(self, d):
        tmp = os.path.join(self.dir, 'ctl.json.tmp')
        with open(tmp, 'w') as f:
            json.dump(d, f)
        os.replace(tmp, os.path.join(self.dir, 'ctl.json'))

    def put(self, uid, raw, ago=0, folder='all'):
        mf.put_eml(os.path.join(self.dir, 'box', folder, '%d.eml' % uid), raw, time.time() - ago)

    def log(self):
        with open(os.path.join(self.dir, 'imap.log')) as f:
            return f.read()

    # --- 辅助程序
    def environ(self, env):
        e = dict(self.env)
        for k, v in env.items():
            if v is None:
                e.pop(k, None)
            else:
                e[k] = v
        return e

    def cca(self, *argv, stdin='', timeout=60, **env):
        """跑一条辅助程序的命令：(退出码, stdout, stderr)。stdin 是交给它的应用专用密码（setup）"""
        r = subprocess.run([HELPER] + list(argv), input=stdin.encode(), stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                           env=self.environ(env), cwd=str(self.home), timeout=timeout)
        return r.returncode, r.stdout.decode('utf-8', 'replace'), r.stderr.decode('utf-8', 'replace')

    def start(self, *argv, **env):
        return Running(self, argv, self.environ(env))

    def watch(self, key='alice@example.com', **env):
        return Watch(self, key, self.environ(env))

    def wait_for(self, cond, what, secs=30):
        end = time.time() + secs
        while time.time() < end:
            v = cond()
            if v:
                return v
            time.sleep(0.02)
        self.fail('等了 %d 秒也没等到：%s' % (secs, what))

    def setup_ok(self, proxy=None):
        code, _, err = self.cca('mail-setup', 'Inbox@Example.com', proxy or self.socks, stdin='abcd efgh ijkl mnop\n')
        self.assertEqual(code, 0, err)
        return err

    # --- 设置和存着的密码
    def config(self):
        return json.loads((self.home / '.cca' / 'config.json').read_text())

    def save(self, c):
        (self.home / '.cca' / 'config.json').write_text(json.dumps(c, ensure_ascii=False, indent=2) + '\n')

    def stored(self, addr):
        """存着的这个地址的应用专用密码（没有是空串）：密钥文件记着地址和密码；假钥匙串一个地址一个文件"""
        if self.keystore == 'file':
            try:
                d = json.loads(self.secret.read_text())
            except OSError:
                return ''
            return d['key'] if d.get('host') == addr else ''
        f = self.home / 'keychain' / 'cca-mail' / addr
        return f.read_text() if f.exists() else ''

    def readable(self, addr):
        """辅助程序自己取不取得到这个地址的密码（原来直接调 ks_read）：把 mail.user 临时换成它，跑 mail-ready"""
        keep = (self.home / '.cca' / 'config.json').read_text()
        c = json.loads(keep)
        c['mail'] = dict(c['mail'], user=addr)
        self.save(c)
        try:
            return self.cca('mail-ready')
        finally:
            (self.home / '.cca' / 'config.json').write_text(keep)

    def mail_list(self, *argv):
        code, out, err = self.cca('gui-mail-list', *argv, CCA_RO='1')
        self.assertEqual((code, err), (0, ''))
        return json.loads(out)


class Wire(Box):
    def test_read_only_session_through_socks_and_http(self):
        self.put(3, mf.build_eml('alice@example.com'), 60)
        for n, proxy in enumerate((self.socks, self.http)):
            # folders() == [('所有邮件', ALL), ('垃圾邮件', '"[Gmail]/Spam"')]：给人看的名字在输出里，IMAP 里的名字在发出去的 EXAMINE 里
            self.assertIn('看这些邮件夹：所有邮件、垃圾邮件', self.setup_ok(proxy))
            d = self.mail_list('', '2')
            self.assertEqual((d['error'], d['folders']), (None, ['所有邮件', '垃圾邮件']))
            self.assertEqual(len(d['rows']), 1)
            row = d['rows'][0]
            # examine(ALL) 的 UIDVALIDITY 是 11、search(…) == [3]、headers([3]) 里有 From、message(3) 里有登录链接
            self.assertEqual(row['msg'], {'folder': ALL, 'uid': 3, 'uidvalidity': 11})
            self.assertEqual((row['from'], row['to']), (['no-reply@mail.anthropic.com'], ['alice@example.com']))
            self.assertTrue(row['link_shape'].startswith('https://claude.ai/magic-link'), row)
            code, out, err = self.cca('mail-test', '2')
            self.assertEqual(code, 0, err)
            self.assertIn('通过核对，链接 https://claude.ai/magic-link（# 后面', out)
            # examine(ALL) 的 UIDNEXT 是 4：盯邮箱的第一轮除了搜到的，还按 UID 取到 UIDNEXT 前一封（1:3）
            code, _, err = self.cca('mail-open', 'alice@example.com', '0', 'again' if n else '')
            self.assertEqual(code, 0, err)
        log = self.log()
        for line in ('IMAP EXAMINE %s' % ALL, 'IMAP EXAMINE %s' % SPAM, 'IMAP UID FETCH 3 (UID INTERNALDATE BODY.PEEK[HEADER.FIELDS (',
                     'IMAP UID FETCH 1:3 (UID INTERNALDATE BODY.PEEK[HEADER.FIELDS (', 'IMAP UID FETCH 3 (UID INTERNALDATE BODY.PEEK[]<0.'):
            self.assertIn(line, log)
        self.assertIn('PROXY socks5 imap.gmail.com:993', log)
        self.assertIn('PROXY http imap.gmail.com:993', log)
        self.assertIn('BODY.PEEK[HEADER.FIELDS', log)
        self.assertNotIn('VIOLATION', log)
        self.assertNotIn(PW, log)
        self.assertEqual({l.split(' ')[1] + (' ' + l.split(' ')[2] if l.split(' ')[1] == 'UID' else '') for l in log.splitlines() if l.startswith('IMAP ')},
                         {'CAPABILITY', 'LOGIN', 'LIST', 'EXAMINE', 'UID SEARCH', 'UID FETCH', 'LOGOUT'}, '只读的会话只有这几种命令')

    def test_login_errors(self):
        # 原来看的是 MailBox.open() 抛的是哪一类错（MailFatal / MailRetry）和它的 code；从外面看：setup 的退出码（3 是密码不对，
        # 重问密码；4 是别的）和说的话各不一样，登录时盯邮箱遇到不会自己好的错马上停、过会儿可能好的接着试
        setup = lambda pw=PW: self.cca('mail-setup', 'inbox@example.com', self.socks, stdin=pw)
        code, _, err = setup('wrongwrongwrongw')   # auth
        self.assertEqual(code, 3, err)
        self.assertIn('Gmail 不认这个密码：要用应用专用密码（16 个字母）', err)
        self.ctl({'imap_disabled': True})   # imap
        code, _, err = setup()
        self.assertEqual(code, 4, err)
        self.assertIn('这个 Gmail 没开 IMAP', err)
        self.ctl({'login_no': '[ALERT] Application-specific password required: https://support.google.com/accounts/answer/185833 (Failure)'})
        code, _, err = setup()   # auth
        self.assertEqual(code, 3, err)
        self.assertIn('Gmail 不认这个密码：要用应用专用密码（16 个字母）', err)
        self.ctl({'login_no': '[UNAVAILABLE] Temporary System Error'})   # 别的 NO：过一会儿再试，带着 Gmail 的原话
        code, _, err = setup()   # login
        self.assertEqual(code, 4, err)
        self.assertIn('Gmail 这次没让登录（[UNAVAILABLE] Temporary System Error）', err)
        self.assertIn('Temporary System Error', err)
        self.assertFalse(self.secret.exists(), '没登录上的都不存')
        # 不会自己好的（MailFatal）：登录时盯邮箱说一句就不再看了
        self.ctl({})
        self.setup_ok()
        for fatal, said in (({'password': 'ponmlkjihgfedcba'}, 'Gmail 不认这个密码'), ({'imap_disabled': True}, '这个 Gmail 没开 IMAP')):
            self.ctl(fatal)
            code, _, err = self.cca('mail-watch', 'alice@example.com', timeout=30)
            self.assertEqual(code, 0)
            self.assertIn(said, err)
            self.assertIn('不再看邮箱（登录照常进行）', err)
        # 过一会儿可能就好了的（MailRetry）：接着试，第三次没连上时说一句
        self.ctl({'login_no': '[UNAVAILABLE] Temporary System Error'})
        w = self.watch()
        self.wait_for(lambda: '还在重试' in w.stderr(), '连不上 Gmail 的那一句')
        err = w.term()
        self.assertIn('连不上 Gmail（Gmail 这次没让登录（[UNAVAILABLE] Temporary System Error）），还在重试', err)
        self.assertNotIn('不再看邮箱', err)

    def test_tls_is_always_verified(self):
        # 不给测试的 CA：假 Gmail 的证书就是「不认识的机构签的」，校验过不去，什么都不存
        for ca in ('', '/nonexistent/ca.pem'):
            code, _, err = self.cca('mail-setup', 'inbox@example.com', self.socks, stdin=PW, CCA_MAIL_CAFILE=ca)
            self.assertEqual(code, 4, err)
            self.assertIn('Gmail 的证书校验没通过', err)
            self.assertIn('cca 不会跳过校验', err)
            self.assertFalse(self.secret.exists())
            self.assertNotIn('IMAP ', self.log(), '握手没成：一条 IMAP 命令都没发出去')
        # 证书校验过不去是不会自己好的（MailFatal，code 是 tls）：登录时盯邮箱说一句就停
        self.setup_ok()
        code, _, err = self.cca('mail-watch', 'alice@example.com', timeout=30, CCA_MAIL_CAFILE='')
        self.assertEqual(code, 0)
        self.assertIn('Gmail 的证书校验没通过', err)
        self.assertIn('不再看邮箱（登录照常进行）', err)
        # 原来的后一半——HOME 是用户真正的主目录时，只给测试的 CA 不认——从外面测不了（辅助程序得在真的主目录下跑）：
        # 认不认的规则在 cli/src/cmd/mail/route.rs 的单元测试 test_ca_only_in_a_swapped_home_and_to_this_machine 里

    def test_proxy_errors_are_retryable(self):
        # 原来看的是 MailRetry 和它的 code（proxy / net）；从外面看：setup 说的话（这两种后面都带「连接方式可以指定」那一句），
        # 登录时盯邮箱接着试、不停
        hint = '连 Gmail 的方式可以指定：cca mail setup --proxy socks5://127.0.0.1:7897（direct 是直连）'
        for mode, proxy, said in (('refuse', self.http, 'HTTP 代理不肯连 imap.gmail.com:993（HTTP/1.1 403 Forbidden）'),
                                  ('auth', self.socks, 'SOCKS5 代理要认证，或者不认这个请求（cca 只用不带认证的 SOCKS5）')):
            self.ctl({'proxy': mode})
            code, _, err = self.cca('mail-setup', 'inbox@example.com', proxy, stdin=PW)
            self.assertEqual(code, 4, mode)
            self.assertIn('%s。%s' % (said, hint), err, mode)   # proxy
        self.ctl({})
        code, _, err = self.cca('mail-setup', 'inbox@example.com', 'socks5://127.0.0.1:9', stdin=PW)
        self.assertEqual(code, 4)
        self.assertIn('连不上 代理 127.0.0.1:9（', err)   # net；括号里是系统的说法（Connection refused）
        self.assertIn('）。%s' % hint, err)
        self.assertFalse(self.secret.exists())
        self.setup_ok()
        for change, said in ((lambda: self.ctl({'proxy': 'refuse'}), 'SOCKS5 代理没能连上 imap.gmail.com:993（错误码 5）'),
                             (lambda: self.ctl({'proxy': 'auth'}), 'SOCKS5 代理要认证'),
                             (lambda: self.save(dict(self.config(), mail={'user': 'inbox@example.com', 'proxy': 'socks5://127.0.0.1:9'})),
                              '连不上 代理 127.0.0.1:9（')):
            change()
            w = self.watch()
            self.wait_for(lambda: '还在重试' in w.stderr(), said)
            err = w.term()
            self.assertIn('连不上 Gmail（%s' % said, err)
            self.assertNotIn('不再看邮箱', err)

    def test_no_all_mail_falls_back_to_inbox_and_odd_fetch_order(self):
        self.ctl({'no_all': True, 'uid_after_literal': True})
        self.put(2, mf.build_eml('alice@example.com'), 10, folder='inbox')
        # folders() == [('收件箱', 'INBOX'), ('垃圾邮件', '"[Gmail]/Spam"')]
        self.assertIn('看这些邮件夹：收件箱、垃圾邮件', self.setup_ok())
        d = self.mail_list('', '2')
        self.assertEqual((d['error'], d['folders']), (None, ['收件箱', '垃圾邮件']))
        self.assertEqual([r['msg'] for r in d['rows']], [{'folder': 'INBOX', 'uid': 2, 'uidvalidity': 11}], 'UID 写在 literal 后面的也认得出')
        # headers((1, 5)) 认出 2：按 UID 的区间取头（盯邮箱第一轮的 1:2），那封照样找到、打开
        code, _, err = self.cca('mail-open', 'alice@example.com', '0')
        self.assertEqual(code, 0, err)
        self.assertEqual(len(self.opened.read_text().splitlines()), 1)
        log = self.log()
        for line in ('IMAP EXAMINE INBOX', 'IMAP EXAMINE %s' % SPAM, 'IMAP UID FETCH 1:2 (UID INTERNALDATE BODY.PEEK[HEADER.FIELDS ('):
            self.assertIn(line, log)
        self.assertNotIn('IMAP EXAMINE %s' % ALL, log)
        self.assertNotIn('VIOLATION', log)

    def test_new_mail_found_even_if_gmail_search_lags(self):
        self.ctl({'index_lag': 3600})   # 刚到的邮件 SEARCH 一律搜不到
        self.setup_ok()
        p = self.start('mail-open', 'alice@example.com', '20')
        # 第一轮什么都没有（poll() == []）：说要等
        self.wait_for(lambda: '等 alice@example.com 的登录邮件' in p.stderr(), '第一轮看完')
        self.assertFalse(self.opened.exists())
        self.put(1, mf.build_eml('alice@example.com', link='https://claude.ai/magic-link#tok-LAG'))
        code, _, err = p.wait()
        self.assertEqual(code, 0, err)
        self.assertEqual(self.opened.read_text(), 'alice@example.com https://claude.ai/magic-link#tok-LAG\n')
        log = self.log()
        self.assertIn('IMAP UID FETCH 1:1 (UID INTERNALDATE BODY.PEEK[HEADER.FIELDS (', log, '不靠搜索：按 UIDNEXT 往后取')
        self.assertNotIn('VIOLATION', log)

    def test_dropped_connection_reconnects(self):
        self.setup_ok()
        open(os.path.join(self.dir, 'imap.log'), 'w').close()
        self.put(1, mf.build_eml('alice@example.com', link='https://claude.ai/magic-link#tok-DROP'), 5)
        self.ctl({'drop': 'UID SEARCH'})
        w = self.watch()
        self.wait_for(lambda: 'DROP UID SEARCH' in self.log(), '这一轮半路断开')   # poll() 抛 MailRetry
        self.assertFalse(self.opened.exists())
        self.ctl({})
        self.wait_for(lambda: self.opened.exists() and self.opened.read_text(), '重连以后找到那一封')   # 再 poll()：[1]
        time.sleep(0.3)
        err = w.term()
        self.assertEqual(self.opened.read_text(), 'alice@example.com https://claude.ai/magic-link#tok-DROP\n')
        self.assertGreaterEqual(self.log().count('IMAP LOGIN'), 2, '断了以后重新登录')
        self.assertIn('已连上 inbox@example.com', err)
        self.assertNotIn('VIOLATION', self.log())


class Commands(Box):
    """cca mail setup / test / open 和登录时盯邮箱：跑辅助程序的 mail-setup、mail-test、mail-open、mail-watch、mail-forget……
    （打开链接的 cca _mail-open 换成记下链接的小脚本）"""

    def test_setup_checks_then_saves(self):
        code, _, err = self.cca('mail-setup', 'inbox@example.com', self.socks, stdin='wrongwrongwrongw')
        self.assertEqual(code, 3)
        self.assertIn('应用专用密码', err)
        self.assertFalse(self.secret.exists(), '没登录上不存')
        self.assertEqual(self.cca('mail-setup', 'not-an-address', '', stdin=PW)[0], 2)
        self.assertEqual(self.cca('mail-setup', 'a;b@example.com', '', stdin=PW)[0], 2)
        self.assertEqual(self.cca('mail-setup', 'inbox@example.com', 'socks5://127.0.0.1:9', stdin=PW)[0], 4)
        err = self.setup_ok()
        self.assertIn('所有邮件、垃圾邮件', err)
        c = self.config()
        self.assertEqual(c['mail'], {'user': 'inbox@example.com', 'proxy': self.socks})
        f = self.secret
        self.assertEqual(stat.S_IMODE(f.stat().st_mode), 0o600)
        self.assertEqual(stat.S_IMODE(f.parent.stat().st_mode), 0o700)
        self.assertEqual(json.loads(f.read_text())['key'], PW, '空格去掉了')
        # ks_read('cca-mail', 'inbox@example.com') == (0, PW)：记的是这个地址和这个密码，辅助程序取得到、拿它登录得上
        self.assertEqual(self.stored('inbox@example.com'), PW)
        self.assertEqual(self.readable('inbox@example.com')[0], 0)
        self.assertEqual(self.cca('mail-test', '1')[0], 0)
        # ks_read('cca-mail', 'other@example.com')[1] == ''
        self.assertEqual(self.stored('other@example.com'), '', '换了地址取不到旧的')
        code, _, err = self.readable('other@example.com')
        self.assertEqual(code, 1)
        self.assertIn('密钥文件里没有应用专用密码', err)
        code, out, _ = self.cca('mail-status')
        self.assertIn('inbox@example.com', out)
        self.assertIn('自动打开', out)
        self.assertEqual(self.cca('mail-get-user')[1], 'inbox@example.com\n')
        self.assertFalse((self.home / 'security.log').exists(), '用密钥文件时不碰钥匙串')

    def test_new_address_replaces_the_old_only_once_saved(self):
        # 原来把 ks_write 换成总是失败的；这里用钥匙串那一支，让假的 security 存不进去（钥匙串锁着）
        self.keystore = 'keychain'
        del self.env['CCA_KEYSTORE']
        self.setup_ok()
        read = self.stored
        user = lambda: self.config()['mail']['user']
        self.assertEqual((read('inbox@example.com'), user()), (PW, 'inbox@example.com'))
        self.ctl({'user': 'new@example.com', 'password': PW})
        (self.home / 'keychain_locked').write_text('')   # 存不进去（钥匙串锁着、磁盘满了）
        code, _, err = self.cca('mail-setup', 'new@example.com', '', stdin=PW)
        self.assertEqual(code, 5, err)
        self.assertIn('应用专用密码没能存进钥匙串（钥匙串锁着？）', err)
        self.assertEqual((read('inbox@example.com'), user()), (PW, 'inbox@example.com'), '原来的密码、设置都还在，照样能用')
        self.assertEqual(self.cca('mail-ready')[0], 0)
        (self.home / 'keychain_locked').unlink()
        self.assertEqual(self.cca('mail-setup', 'new@example.com', '', stdin=PW)[0], 0)
        self.assertEqual((read('new@example.com'), read('inbox@example.com'), user()), (PW, '', 'new@example.com'))
        self.assertEqual(self.cca('mail-ready')[0], 0)

    def test_setup_when_gmail_refuses_for_now(self):
        self.ctl({'login_no': '[UNAVAILABLE] Temporary System Error'})
        code, _, err = self.cca('mail-setup', 'inbox@example.com', self.socks, stdin=PW)
        self.assertEqual(code, 4, '不是密码不对：不重问密码')
        self.assertIn('Gmail 这次没让登录（[UNAVAILABLE] Temporary System Error）', err)
        self.assertNotIn('连 Gmail 的方式可以指定', err)

    def test_setup_keeps_auto_off(self):
        self.save({'mail': {'user': 'inbox@example.com', 'auto': False}})
        self.setup_ok()
        self.assertIs(self.config()['mail']['auto'], False)

    def test_mail_test_lists_without_showing_links(self):
        self.setup_ok()
        self.put(5, mf.build_eml('alice@example.com', link='https://claude.ai/magic-link#tok-SECRET1'), 120)
        self.put(6, mf.build_eml('carol@example.com', link='https://claude.ai/magic-link#tok-SECRET2'), 60)
        self.put(7, mf.build_eml('alice@example.com', ar='fail', link='https://claude.ai/magic-link#tok-SECRET3'), 30)
        self.put(8, mf.build_eml('alice@example.com', subject='Secure link to log in to Claude.ai \x1b[2K\x1b[1A', ar='fail',
                                 frm='"\x1b]52;c;eA==\x07" <x@mail.anthropic.com>'), 20)
        code, out, err = self.cca('mail-test', '2')
        self.assertEqual(code, 0, err)
        self.assertNotIn('SECRET', out + err)
        self.assertNotIn('\x1b', out, '发件人写的转义序列不原样打到终端')
        self.assertIn('有 4 封', out)
        self.assertIn('通过核对，链接 https://claude.ai/magic-link（# 后面', out)
        self.assertIn('不是账号表里的邮箱', out)
        self.assertIn('Gmail 的验证结果里没有 Anthropic 的签名通过', out)
        self.assertNotIn('VIOLATION', self.log())
        # App 的邮件列表（_gui mail-list）也一样：只给链接的样子
        d = self.mail_list('', '2')
        self.assertEqual(len(d['rows']), 4)
        self.assertNotIn('SECRET', json.dumps(d))

    def test_open_hands_the_newest_link_over_stdin_once(self):
        self.setup_ok()
        self.put(5, mf.build_eml('alice@example.com', link='https://claude.ai/magic-link#tok-A1'), 600)
        self.put(6, mf.build_eml('alice@example.com', link='https://claude.ai/magic-link#tok-A2'), 300)
        self.put(7, mf.build_eml('alice@example.com', link='https://claude.ai/magic-link#tok-OLD'), 3 * 3600)
        code, _, err = self.cca('mail-open', 'alice@example.com', '0')
        self.assertEqual(code, 0, err)
        self.assertEqual(self.opened.read_text(), 'alice@example.com https://claude.ai/magic-link#tok-A2\n')
        code, _, err = self.cca('mail-open', 'alice@example.com', '0')
        self.assertEqual(code, 1)
        self.assertIn('已经打开过了', err)
        code, _, err = self.cca('mail-open', 'alice@example.com', '0', 'again')
        self.assertEqual(code, 0, err)
        self.assertEqual(self.opened.read_text().count('tok-A2'), 2)
        self.assertNotIn('tok-', err)
        code, _, err = self.cca('mail-open', '工作(bob@example.com)', '0')
        self.assertEqual(code, 1)
        self.assertIn('最近 30 分钟里没有 bob@example.com 的登录邮件', err)
        self.assertNotIn('VIOLATION', self.log())
        # 链接只经 stdin 给 cca _mail-open：命令行上只有 _mail-open 和账号，环境变量里没有它；也不存盘（used.json 只记摘要）
        self.assertEqual(set(self.seen.read_text().splitlines()), {'2 0'})
        mail = self.home / '.cca' / 'mail'
        self.assertEqual((stat.S_IMODE(mail.stat().st_mode), stat.S_IMODE((mail / 'used.json').stat().st_mode)), (0o700, 0o600))
        for f in mail.iterdir():
            self.assertNotIn('magic', f.read_text(), f.name)
            self.assertNotIn('tok-', f.read_text(), f.name)

    def test_open_waits_for_a_new_mail(self):
        self.setup_ok()
        p = self.start('mail-open', 'alice@example.com', '20')
        self.wait_for(lambda: '等 alice@example.com 的登录邮件' in p.stderr(), '第一轮看完、说要等')
        self.put(9, mf.build_eml('alice@example.com', link='https://claude.ai/magic-link#tok-W'))
        code, _, err = p.wait()
        self.assertEqual(code, 0, err)
        self.assertIn('等 alice@example.com 的登录邮件', err)
        self.assertIn('tok-W', self.opened.read_text())

    def test_open_reports_browser_failure(self):
        self.setup_ok()
        self.rc.write_text('3')
        self.put(5, mf.build_eml('alice@example.com'), 10)
        code, _, err = self.cca('mail-open', 'alice@example.com', '0')
        self.assertEqual(code, 1)
        self.assertIn('没能打开浏览器', err)
        self.assertEqual(len(self.opened.read_text().splitlines()), 3)
        self.rc.write_text('0')
        self.assertEqual(self.cca('mail-open', 'alice@example.com', '0')[0], 0, '没用掉：修好后照样能开')

    def test_watch_opens_the_mail_that_arrives_during_login(self):
        self.setup_ok()
        self.put(4, mf.build_eml('alice@example.com', link='https://claude.ai/magic-link#tok-before'), 120)
        w = self.watch()
        self.wait_for(lambda: '已连上 inbox@example.com' in w.stderr(), '连上 Gmail')
        time.sleep(0.3)
        self.put(5, mf.build_eml('bob@example.com', link='https://claude.ai/magic-link#tok-bob'))
        time.sleep(0.3)
        self.put(6, mf.build_eml('alice@example.com', link='https://claude.ai/magic-link#tok-login'))
        self.wait_for(lambda: self.opened.exists() and 'tok-login' in self.opened.read_text(), '打开登录邮件')
        time.sleep(0.5)
        err = w.over()   # 登录结束（原来是 alive() 变假）
        self.assertEqual(self.opened.read_text(), 'alice@example.com https://claude.ai/magic-link#tok-login\n',
                         '登录开始前就到了的、别的账号的都不开')
        self.assertIn('已连上 inbox@example.com', err)
        self.assertNotIn('没通过核对', err, '别的账号的登录邮件不说')
        self.assertIn('打开了登录邮件里的链接', json.loads((self.home / '.cca' / 'mail' / 'last.json').read_text())['text'])

    def test_watch_does_nothing_without_config_or_when_auto_is_off(self):
        self.assertEqual(self.cca('mail-watch', 'alice@example.com', timeout=30), (0, '', ''))   # 没配置：马上返回
        self.setup_ok()
        c = self.config()
        c['mail']['auto'] = False
        self.save(c)
        self.assertEqual(self.cca('mail-watch', 'alice@example.com', timeout=30), (0, '', ''))
        self.assertNotIn('LOGIN', self.log().split('IMAP LOGOUT')[-1])

    def test_forget_removes_everything(self):
        self.setup_ok()
        # 原来直接调 mail_claim('x') 建出 ~/.cca/mail；这里真打开一封（used.json、last.json 都在里面）
        self.put(5, mf.build_eml('alice@example.com'), 10)
        self.assertEqual(self.cca('mail-open', 'alice@example.com', '0')[0], 0)
        self.assertTrue((self.home / '.cca' / 'mail' / 'used.json').exists())
        self.assertEqual(self.cca('mail-forget')[0], 0)
        self.assertFalse(self.secret.exists())
        self.assertFalse((self.home / '.cca' / 'mail').exists())
        self.assertNotIn('mail', self.config())
        self.assertEqual(self.cca('mail-forget')[0], 0, '没配置过也照样能跑')


class Keystore(Box):
    """ks_read / ks_write / ks_del 走钥匙串的那一支：假的 security 把条目放在 ~/keychain 里（和 tests/setup.sh 的一样认 -a、-s、-X）。
    原来在进程里直接调这三个函数（那一条在 cli/src/cmd/secrets.rs 的单元测试里）；这里经 mail-setup / mail-forget 走真的进程"""
    keystore = 'keychain'

    def entries(self):
        d = self.home / 'keychain' / 'cca-mail'
        return {f.name: f.read_text() for f in d.iterdir()} if d.exists() else {}

    def test_write_replaces_the_same_account_only_and_delete_can_pick_one(self):
        a, b = 'abcdefghijklmnop', 'ponmlkjihgfedcba'
        setup = lambda user, pw, acct='': (self.ctl({'user': user, 'password': pw}),
                                           self.cca('mail-setup', user, self.socks, acct, stdin=pw))[1]
        self.assertEqual(setup('a@example.com', a)[0], 0)                        # ks_write('svc', 'a@example.com', a)
        self.assertEqual(setup('b@example.com', b, 'alice@example.com')[0], 0)   # ks_write('svc', 'b@example.com', b)
        self.assertEqual(self.entries(), {'a@example.com': a, 'b@example.com': b})
        self.assertEqual(setup('b@example.com', a, 'alice@example.com')[0], 0)   # ks_write('svc', 'b@example.com', a)：只换同一个账户名的
        self.assertEqual(self.entries(), {'a@example.com': a, 'b@example.com': a})
        # ks_read：两个地址各自取得到（默认的那个、alice 单独用的那个）
        self.assertEqual((self.cca('mail-ready')[0], self.cca('mail-ready', 'alice@example.com')[0]), (0, 0))
        self.assertEqual(self.cca('mail-forget', 'default')[0], 0)               # ks_del('svc', 'a@example.com')
        self.assertEqual(set(self.entries()), {'b@example.com'}, '只删这个账户名的')
        self.assertEqual(self.cca('mail-forget')[0], 0)                          # ks_del('svc')
        self.assertEqual(self.entries(), {})
        calls = (self.home / 'security.log').read_text()
        self.assertIn('SECURITY -i\n', calls)
        self.assertIn('find-generic-password -a b@example.com -s cca-mail -w', calls)
        for secret in (a, b, a.encode().hex(), b.encode().hex()):
            self.assertNotIn(secret, calls, '秘密不进命令行')
        self.assertIn(a.encode().hex(), (self.home / 'security.stdin').read_text(), '只从 stdin 给 security -i')
        self.assertFalse(self.secret.exists(), '用钥匙串时不写密钥文件')


if __name__ == '__main__':
    unittest.main()
