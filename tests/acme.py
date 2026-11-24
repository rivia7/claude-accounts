#!/usr/bin/env python3
"""证书助手（server/cca-acme.py：给网页端申请、续上浏览器信得过的证书）对着假的证书机构（tests/acme_fake.py：照 ACME 的规范逐项核对
请求，http-01 是真的来取文件）。看：给 IP 地址要的是短期的那一种、nonce 不对时自己再发、证书和私钥放对地方而且权限对、没到时候不重复要、
到时候续上而私钥不换、各种没成的情况说得清楚而且不留下半截的东西；几个 Team 共用的那种（--teams、--store）：同一个地址只要一张、
登记的地址不对的跳过、各个 Team 的那一份权限对、手里已经有的（以前它自己的证书助手要的）接着用、运行着时登记了新的马上去要。
不联网（都在 127.0.0.1 上）。"""
import http.client
import json
import os
from pathlib import Path
import socket
import stat
import subprocess
import sys
import tempfile
import time
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from helper_bin import ACME_ARGV   # noqa: E402  证书助手用哪一份跑（平常是 server/cca-acme.py）
from presence import ROOT, wait_for   # noqa: E402

FAKE = ROOT / 'tests' / 'acme_fake.py'


def run(*args):
    return subprocess.run(['openssl'] + [str(a) for a in args], check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE).stdout


def free_port():
    s = socket.socket()
    s.bind(('127.0.0.1', 0))
    port = s.getsockname()[1]
    s.close()
    return port


class AcmeTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='cca-acme-')
        self.addCleanup(self.tmp.cleanup)
        d = self.d = Path(self.tmp.name).resolve()
        ca = self.cad = d / 'ca'
        ca.mkdir()
        # 测试的证书机构（带着“能签证书”的用途：新的 Python 默认连这个也查），和假证书机构自己的 HTTPS 证书
        run('req', '-x509', '-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:prime256v1', '-nodes', '-days', '2', '-subj', '/CN=cca test ca',
            '-addext', 'basicConstraints=critical,CA:TRUE', '-addext', 'keyUsage=critical,keyCertSign,cRLSign', '-keyout', ca / 'ca.key', '-out', ca / 'ca.pem')
        run('req', '-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:prime256v1', '-nodes', '-subj', '/CN=localhost', '-keyout', ca / 'key.pem', '-out', ca / 'csr.pem')
        (ca / 'ext.cnf').write_text('subjectAltName=IP:127.0.0.1\nbasicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\nkeyUsage=critical,digitalSignature\n')
        run('x509', '-req', '-in', ca / 'csr.pem', '-CA', ca / 'ca.pem', '-CAkey', ca / 'ca.key', '-CAcreateserial', '-days', '2', '-extfile', ca / 'ext.cnf',
            '-out', ca / 'cert.pem')
        self.http_port = free_port()

    def authority(self, *flags, http_port=None):
        """起假的证书机构；返回它的目录地址"""
        (self.cad / 'port').unlink(missing_ok=True)
        p = subprocess.Popen([sys.executable, '-I', str(FAKE), str(self.cad), str(http_port or self.http_port), *flags], stdin=subprocess.DEVNULL,
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.addCleanup(lambda: (p.kill(), p.wait()))
        port = int(wait_for('假的证书机构起来', lambda: (self.cad / 'port').exists() and (self.cad / 'port').read_text()))
        return 'https://127.0.0.1:%d/dir' % port

    def acme(self, directory, *args, ids=('127.0.0.1',), once=True, any_ip=True):
        argv = ACME_ARGV + ['--out', str(self.d / 'cert'), '--account', str(self.d / 'account'),
                '--listen', '127.0.0.1:%d' % self.http_port, '--directory', directory, '--ca', str(self.cad / 'ca.pem'), *args]
        for i in ids:
            argv += ['--id', i]
        if any_ip:
            argv.append('--any-ip')
        if once:
            argv.append('--once')
        return subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=180)

    def log(self):
        p = self.cad / 'log'
        return [json.loads(line) for line in p.read_text().splitlines()] if p.exists() else []

    def cert(self):
        text = run('x509', '-in', self.d / 'cert' / 'fullchain.pem', '-noout', '-subject', '-ext', 'subjectAltName', '-serial', '-pubkey').decode()
        return text

    def mode(self, *parts):
        return stat.S_IMODE(os.stat(self.d.joinpath(*parts)).st_mode)

    def test_an_ip_certificate_and_its_renewal(self):
        directory = self.authority('--bad-nonce', '3')
        r = self.acme(directory)
        self.assertEqual((r.returncode, r.stdout), (0, ''), r.stderr)
        self.assertIn('拿到 127.0.0.1 的证书', r.stderr)
        log = self.log()
        (order,) = [e for e in log if 'order' in e]
        self.assertEqual((order['ids'], order['profile']), ([{'type': 'ip', 'value': '127.0.0.1'}], 'shortlived'), '给 IP 签的要的是短期的那一种')
        self.assertEqual([(e['validated'], e['ok']) for e in log if 'validated' in e], [('127.0.0.1', True)])
        self.assertTrue([e for e in log if e.get('refused') == 'badNonce'], '说 nonce 不对的那几次，它自己再发了')
        self.assertEqual([e for e in log if 'refused' in e and e['refused'] != 'badNonce'], [])
        # 证书：给 127.0.0.1 的、没有 Common Name、连着签它的那一张；照测试的机构验得过；私钥和它是一对
        text = self.cert()
        self.assertIn('IP Address:127.0.0.1', text)
        self.assertNotIn('CN', text.split('\n')[0])
        chain = (self.d / 'cert' / 'fullchain.pem').read_text()
        self.assertEqual(chain.count('BEGIN CERTIFICATE'), 2)
        (self.d / 'leaf.pem').write_text(chain[:chain.index('-----END CERTIFICATE-----') + 26])
        run('verify', '-CAfile', self.cad / 'ca.pem', self.d / 'leaf.pem')
        pub = lambda: run('pkey', '-in', self.d / 'cert' / 'privkey.pem', '-pubout')
        self.assertIn(pub().decode().strip(), text)
        # 权限：两个目录只有自己进得去，两把私钥只有自己读得了，证书别人读得了
        self.assertEqual([self.mode('cert'), self.mode('account'), self.mode('cert', 'privkey.pem'), self.mode('account', 'account.key'),
                          self.mode('cert', 'fullchain.pem')], [0o700, 0o700, 0o600, 0o600, 0o644])
        self.assertEqual(sorted(p.name for p in (self.d / 'cert').iterdir()), ['fullchain.pem', 'privkey.pem'], '没有留下半截的文件')
        # 还没到续的时候：再运行一次什么都不向证书机构要
        before, serial, key = len(self.log()), text.split('serial=')[1].split()[0], pub()
        r = self.acme(directory)
        self.assertEqual((r.returncode, r.stderr), (0, ''))
        self.assertEqual(len(self.log()), before)
        # 到时候了（这里说“一过有效期的开头就续”）：要一张新的，用的还是同一个账户、同一把证书私钥
        r = self.acme(directory, '--renew-at', '0')
        self.assertEqual(r.returncode, 0, r.stderr)
        log = self.log()
        self.assertEqual([e['new'] for e in log if 'account' in e], [True, False], '续的时候是原来的账户')
        self.assertEqual(len([e for e in log if 'issued' in e]), 2)
        self.assertNotEqual(self.cert().split('serial=')[1].split()[0], serial)
        self.assertEqual(pub(), key, '证书的私钥没换')

    def test_a_domain(self):
        directory = self.authority()
        r = self.acme(directory, ids=('Relay.Example.com',), any_ip=False)
        self.assertEqual(r.returncode, 0, r.stderr)
        (order,) = [e for e in self.log() if 'order' in e]
        self.assertEqual((order['ids'], order['profile']), ([{'type': 'dns', 'value': 'relay.example.com'}], 'shortlived'))
        self.assertIn('DNS:relay.example.com', self.cert())

    def test_what_goes_wrong(self):
        # 不是公网的地址：证书机构不会给签，不去要
        r = self.acme('https://127.0.0.1:1/dir', ids=('192.168.1.5',), any_ip=False)
        self.assertEqual(r.returncode, 2)
        self.assertIn('192.168.1.5 不是公网的地址', r.stderr)
        r = self.acme('https://127.0.0.1:1/dir', ids=('not a name',), any_ip=False)
        self.assertEqual(r.returncode, 2)
        # 连不上证书机构
        r = self.acme('https://127.0.0.1:1/dir')
        self.assertEqual(r.returncode, 1)
        self.assertIn('连不上证书机构', r.stderr)
        self.assertIn('手里没有用得了的证书', r.stderr)
        # 证书机构从“公网”连不上应答的端口（80 端口没放行）：说是验证没过，不留下证书
        directory = self.authority(http_port=free_port())
        r = self.acme(directory)
        self.assertEqual(r.returncode, 1)
        self.assertIn('验证这个地址是你的（证书机构从公网连 80 端口）没过', r.stderr)
        self.assertFalse((self.d / 'cert' / 'fullchain.pem').exists())
        self.assertEqual([e['ok'] for e in self.log() if 'validated' in e], [False])

    def test_no_short_lived_profile(self):
        directory = self.authority('--no-shortlived')
        r = self.acme(directory)
        self.assertEqual(r.returncode, 1)
        self.assertIn('这个证书机构没有给 IP 地址签的那种证书（shortlived）', r.stderr)
        self.assertEqual([e for e in self.log() if 'order' in e], [], '没有那一种就不下单')
        # 域名的：证书机构没有 shortlived 时不写种类，照它默认的
        r = self.acme(directory, ids=('relay.example.com',), any_ip=False)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual([e['profile'] for e in self.log() if 'order' in e], [None])

    def test_webroot(self):
        """80 端口上已经有别的网站服务：不自己听端口，验证的文件写进一个目录，由那个服务给出去；验完就删"""
        root = self.d / 'site'
        root.mkdir()
        (root / 'index.html').write_text('别的网站')
        site = subprocess.Popen([sys.executable, '-I', '-m', 'http.server', str(self.http_port), '--bind', '127.0.0.1', '--directory', str(root)],
                                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.addCleanup(lambda: (site.kill(), site.wait()))

        def up():
            try:
                socket.create_connection(('127.0.0.1', self.http_port), 1).close()
                return True
            except OSError:
                return False
        wait_for('那个网站起来', up)
        directory = self.authority()
        r = self.acme(directory, '--webroot', str(root))
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual([(e['validated'], e['ok']) for e in self.log() if 'validated' in e], [('127.0.0.1', True)])
        self.assertIn('IP Address:127.0.0.1', self.cert())
        held = root / '.well-known' / 'acme-challenge'
        self.assertEqual((held.is_dir(), list(held.iterdir())), (True, []), '验证的文件验完就删了')
        self.assertEqual((root / 'index.html').read_text(), '别的网站')

    def teams(self, directory, *args, once=True):
        """几个 Team 共用的证书助手（--teams）：登记在 teams/，证书存在 store/，放进 out/<服务名>/"""
        argv = ACME_ARGV + ['--teams', str(self.d / 'teams'), '--store', str(self.d / 'store'), '--out', str(self.d / 'out'),
                '--account', str(self.d / 'account'), '--listen', '127.0.0.1:%d' % self.http_port, '--directory', directory,
                '--ca', str(self.cad / 'ca.pem'), '--any-ip', *args] + (['--once'] if once else [])
        if not once:
            p = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
            self.addCleanup(lambda: (p.kill(), p.wait(), p.stderr.close()))
            return p
        return subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=180)

    def register(self, team, address, outdir=True):
        (self.d / 'teams').mkdir(exist_ok=True)
        (self.d / 'teams' / team).write_text(address + '\n')
        if outdir:
            (self.d / 'out' / team).mkdir(parents=True, exist_ok=True)

    def test_several_teams(self):
        """几个 Team 共用一个：每个地址只要一张（两个 Team 用同一个地址也是一张），放进各自的目录（私钥 640：组是那个 Team 的）；
        目录还没建的那个 Team 说一声；建了以后不再要、只放进去；看不懂的登记跳过；手里没有、Team 的目录里有用得了的（以前它自己的证书助手
        要的）就接着用它，不再要一张"""
        directory = self.authority()
        a, b, c = 'cca-team-aaaaaaaa', 'cca-team-bbbbbbbb', 'cca-team-cccccccc'
        self.register(a, '127.0.0.1')
        self.register(b, '127.0.0.1')
        self.register(c, 'Relay.Example.com', outdir=False)
        self.register('cca-team-dddddddd', 'not a name')
        r = self.teams(directory)
        self.assertEqual(r.returncode, 1, r.stderr)
        self.assertIn('cca-team-dddddddd 登记的地址用不了', r.stderr)
        self.assertIn('%s 的目录 %s 不在' % (c, self.d / 'out' / c), r.stderr)
        orders = [e['ids'] for e in self.log() if 'order' in e]
        self.assertEqual(sorted(i[0]['value'] for i in orders), ['127.0.0.1', 'relay.example.com'], '一个地址一张，两个 Team 共用 127.0.0.1 的那张')
        for t in (a, b):
            for f in ('fullchain.pem', 'privkey.pem'):
                self.assertEqual((self.d / 'out' / t / f).read_bytes(), (self.d / 'store' / 'ip-127.0.0.1' / f).read_bytes(), t + f)
            self.assertEqual([self.mode('out', t, 'privkey.pem'), self.mode('out', t, 'fullchain.pem')], [0o640, 0o644])
            self.assertEqual(sorted(p.name for p in (self.d / 'out' / t).iterdir()), ['fullchain.pem', 'privkey.pem'], '没有留下半截的文件')
        self.assertEqual([self.mode('store'), self.mode('store', 'ip-127.0.0.1'), self.mode('store', 'ip-127.0.0.1', 'privkey.pem')], [0o700, 0o700, 0o600])
        self.assertIn('DNS:relay.example.com', run('x509', '-in', self.d / 'store' / 'dns-relay.example.com' / 'fullchain.pem', '-noout', '-ext',
                                                   'subjectAltName').decode())
        # c 的目录建好了：不再要，只放进去
        (self.d / 'out' / c).mkdir()
        n = len(self.log())
        r = self.teams(directory)
        self.assertEqual((r.returncode, len(self.log())), (0, n), r.stderr)
        self.assertEqual((self.d / 'out' / c / 'fullchain.pem').read_bytes(), (self.d / 'store' / 'dns-relay.example.com' / 'fullchain.pem').read_bytes())
        # 存的那一份没了（新装的证书助手），a 的目录里有用得了的：接着用它，不再要一张
        (self.d / 'store').rename(self.d / 'store.old')
        (self.d / 'teams' / c).unlink()
        r = self.teams(directory)
        self.assertEqual((r.returncode, len(self.log())), (0, n), r.stderr)
        self.assertIn('127.0.0.1 接着用 %s 原来的证书' % a, r.stderr)
        self.assertEqual((self.d / 'store' / 'ip-127.0.0.1' / 'privkey.pem').read_bytes(), (self.d / 'out' / a / 'privkey.pem').read_bytes())
        self.assertFalse((self.d / 'store' / 'dns-relay.example.com').exists(), '没人用的地址不要')

    def test_several_teams_running(self):
        """一直运行：登记的变了（安装脚本加了一个 Team）过几秒就照新的，不用重启"""
        directory = self.authority()
        self.register('cca-team-aaaaaaaa', '127.0.0.1')
        p = self.teams(directory, once=False)
        wait_for('第一个 Team 的证书', lambda: (self.d / 'out' / 'cca-team-aaaaaaaa' / 'fullchain.pem').exists())
        self.register('cca-team-bbbbbbbb', '127.0.0.1')
        wait_for('第二个 Team 的证书', lambda: (self.d / 'out' / 'cca-team-bbbbbbbb' / 'fullchain.pem').exists(), 30)
        self.assertIsNone(p.poll())
        self.assertEqual(len([e for e in self.log() if 'issued' in e]), 1, '同一个地址：还是那一张')

    def test_the_responder_and_running_forever(self):
        """不带 --once：拿到证书以后接着运行（等着续）；应答的端口上别的路径都是 404"""
        directory = self.authority()
        argv = ACME_ARGV + ['--out', str(self.d / 'cert'), '--account', str(self.d / 'account'), '--listen', '127.0.0.1:%d' % self.http_port,
                '--directory', directory, '--ca', str(self.cad / 'ca.pem'), '--id', '127.0.0.1', '--any-ip', '--port-file', str(self.d / 'aport')]
        p = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
        self.addCleanup(lambda: (p.kill(), p.wait(), p.stderr.close()))
        wait_for('证书', lambda: (self.d / 'cert' / 'fullchain.pem').exists())
        self.assertEqual(int((self.d / 'aport').read_text()), self.http_port)
        for path in ('/', '/.well-known/acme-challenge/', '/.well-known/acme-challenge/nope', '/.well-known/acme-challenge/../../etc/passwd',
                     '/privkey.pem', '/.well-known/acme-challenge/' + 'a' * 300):
            c = http.client.HTTPConnection('127.0.0.1', self.http_port, timeout=10)
            c.request('GET', path)
            r = c.getresponse()
            self.assertEqual((r.status, r.read()), (404, b''), path)
            c.close()
        time.sleep(1)
        self.assertIsNone(p.poll(), '拿到证书以后接着运行')
        self.assertEqual(len([e for e in self.log() if 'issued' in e]), 1)


if __name__ == '__main__':
    unittest.main(verbosity=2)
