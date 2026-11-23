#!/usr/bin/env python3
"""Team 的同步服务器的一键部署（docs/dev/team.md「部署」）。辅助程序生成的安装脚本（hub-installer）在 tests/hub_box.py 的假服务器上运行
（临时目录当作 /，假的 systemctl、docker 起真的 server/cca-presence.py；证书助手换成假的，照 --id 写一张自签的证书）。有 /bin/dash
（Debian、Ubuntu 的 sh）就用它运行，有的用例用 /bin/sh（macOS、Fedora 上是 bash，Alpine 上是 busybox）；tests/linux.sh 在各个发行版的
容器里也跑这一份。装好以后管理员的设备（tests/presence.py 的 Device：临时 HOME、假 curl）用辅助程序真的接上、发邀请，别的设备加入。
cca team deploy 的那一半（team-deploy）经假的 ssh 在同一台假服务器上运行同一个脚本。"""
import http.client
import json
import os
from pathlib import Path
import secrets
import shlex
import shutil
import signal
import socket
import ssl
import subprocess
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from helper_bin import HELPER   # noqa: E402
from hub_box import SH, SS, UFW, Box, free_port   # noqa: E402
from presence import Device, openssl_pin, wait_for   # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
TEMPLATE = ROOT / 'server' / 'hub-install.sh'
WEB = ROOT / 'server' / 'web'
IMAGES = ['public.ecr.aws/docker/library/python:3.13-slim', 'mirror.ccs.tencentyun.com/library/python:3.13-slim']
UNITS = Path('etc') / 'systemd' / 'system'
ASVC = 'cca-team-acme'   # 这台服务器上几个 Team 共用的证书助手（/opt/cca-team-acme）
# 设备上的 ssh：-G 说这台假服务器在 127.0.0.1（$HUB_SSH_HOST 换一个：ssh 走的是内网地址）；别的把最后一个参数（在服务器上运行的命令）
# 交给假服务器的 enter（stdin 是安装脚本）
SSH = r'''printf '%s\n' "$*" >> "$HOME/ssh.argv"
for a in "$@"; do [ "$a" = -G ] && { printf 'user root\nhostname %s\nport 22\n' "${HUB_SSH_HOST:-127.0.0.1}"; exit 0; }; done
for a in "$@"; do cmd=$a; done
exec "$HUB_ENTER" "$cmd"
'''
# 要密码的服务器（$HUB_PW 文件里是密码）：不认这台设备的密钥；用密码登录时像真的 ssh 一样经 SSH_ASKPASS 取密码（SSH_ASKPASS_REQUIRE=force），
# 每次连接取一次（$HOME/askpass.n 记着取了几次）
SSH_PW = r'''printf '%s\n' "$*" >> "$HOME/ssh.argv"
for a in "$@"; do [ "$a" = -G ] && { printf 'user root\nhostname 127.0.0.1\nport 22\n'; exit 0; }; done
case " $* " in
	*" PubkeyAuthentication=no "*)
		[ "${SSH_ASKPASS_REQUIRE:-}" = force ] && [ -n "${SSH_ASKPASS:-}" ] || { echo 'root@box: Permission denied (no askpass).' >&2; exit 255; }
		pw=$("$SSH_ASKPASS") || exit 255
		echo x >> "$HOME/askpass.n"
		[ "$pw" = "$(cat "$HUB_PW")" ] || { echo 'root@box: Permission denied (publickey,password).' >&2; exit 255; } ;;
	*) echo 'root@box: Permission denied (publickey,password).' >&2; exit 255 ;;
esac
for a in "$@"; do cmd=$a; done
exec "$HUB_ENTER" "$cmd"
'''
# ssh 连不上、主机密钥不对……：stderr 是 $HUB_SSH_SAYS
SSH_SAYS = r'''printf '%s\n' "$*" >> "$HOME/ssh.argv"
for a in "$@"; do [ "$a" = -G ] && { printf 'user root\nhostname 127.0.0.1\nport 22\n'; exit 0; }; done
printf '%s\n' "$HUB_SSH_SAYS" >&2
exit 255
'''


def pin(d):
    """装在 d 里的那个 Team 的服务器的证书公钥的指纹（sha256//…）"""
    return openssl_pin(d / 'certs' / 'cert.pem')


def kv(out):
    """team-deploy 成了时的输出：sh 的 键=值，一行一项"""
    return dict(shlex.split(l)[0].split('=', 1) for l in out.splitlines() if l)


def https(port, path='/v1/health'):
    """GET https://127.0.0.1:端口/路径（不验证书：网页端的证书是假的证书助手自签的）：(状态码, 内容)"""
    c = http.client.HTTPSConnection('127.0.0.1', port, context=ssl._create_unverified_context(), timeout=10)
    try:
        c.request('GET', path)
        r = c.getresponse()
        return r.status, r.read()
    finally:
        c.close()


def health(port):
    st, body = https(port)
    assert st == 200, st
    return json.loads(body)


def two_ports():
    """HTTPS 的端口和网页端的端口：都空着；网页端的不是中转自动选的那一个（HTTPS 的下一个，free_port 常常接着给）"""
    port, web = free_port(), free_port()
    while web in (port, port + 1):
        web = free_port()
    return port, web


def apart(n):
    """n 个 Team 各自的 (HTTPS 的端口, 网页端的端口)：都空着，互不相同，也不是谁的中转自动选的那一个（HTTPS 的下一个）"""
    out, used = [], set()
    while len(out) < n:
        port, web = two_ports()
        mine = {port, port + 1, web}
        if not mine & used and len(mine) == 3:
            out.append((port, web))
            used |= mine
    return out


def port_free(port):
    with socket.socket() as s:
        try:
            s.bind(('127.0.0.1', port))
            return True
        except OSError:
            return False


class Base(unittest.TestCase):
    """生成一次安装脚本；每个用例自己的假服务器、设备"""
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix='cca-hub-deploy-')
        cls.t = Path(cls.tmp.name).resolve()
        r = subprocess.run([HELPER, 'hub-installer'], env={'HOME': str(cls.t), 'PATH': '/usr/bin:/bin', 'CCA_VERSION': '9.9.9'},
                           capture_output=True, text=True, timeout=30)
        assert r.returncode == 0 and r.stderr == '', r
        cls.script = cls.t / 'cca-hub-install.sh'
        cls.script.write_text(r.stdout)

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def box(self, **kw):
        b = Box(self.t / ('box-' + secrets.token_hex(4)), **kw)
        self.addCleanup(b.stop)
        return b

    def admin(self, name='admin'):
        """一台设备（没打开维护者的共享），设备上的 ssh 是假的（连到 HUB_ENTER 那台假服务器）"""
        dev = Device(self.t / ('%s-%s' % (name, secrets.token_hex(4))), name, {})
        dev.config(lambda c: c.pop('policy'))
        dev.fake('ssh', SSH)
        return dev

    def new_team(self, dev, name):
        """dev 建一个 Team（它是管理员）：编号、根公钥、服务名"""
        tid, root = dev.ok('team-new', name, '--device', 'studio').split('\n')[:2]
        return tid, root, 'cca-team-' + tid[:8]

    def team(self, name='甲组'):
        """管理员、它建的 Team：(设备, 编号, 根公钥, 服务名)"""
        a = self.admin()
        return (a,) + self.new_team(a, name)

    def install(self, b, *args, shell=SH, env=None):
        """在服务器上直接运行（sudo sh cca-hub-install.sh …），该成的"""
        r = b.install(self.script, *args, shell=shell, env=env)
        self.assertEqual((r.returncode, r.stderr), (0, ''), r.stdout)
        return r

    def code(self, dev, *argv, **kw):
        r = dev.run(*argv, **kw)
        self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
        return r.stdout.split('\t')[0]


class InstallerTest(Base):
    """在服务器上直接运行安装脚本（sudo sh cca-hub-install.sh --team-root …）"""
    def test_script_itself(self):
        """生成的脚本：sh -n 过得去、带着版本、服务端程序和证书助手；模板本身不能运行；参数、root 的检查（什么都不动）"""
        s = self.script.read_text()
        for sh in sorted({SH, '/bin/sh'}):
            self.assertEqual(subprocess.run([sh, '-n', str(self.script)]).returncode, 0, sh)
        self.assertIn("CCA_HUB_VERSION='9.9.9'", s)
        for f in ('cca-presence.py', 'cca-acme.py'):
            self.assertIn((ROOT / 'server' / f).read_text(), s)
        self.assertNotRegex(s, r'(?m)^CCA_HUB_MANAGED=1$', '直接在服务器上运行的那一份不带 team-deploy 放进的那几项')
        b = self.box()
        r = b.install(TEMPLATE)
        self.assertEqual(r.returncode, 1)
        self.assertIn('模板', r.stderr)
        a, tid, root, svc = self.team()
        r = b.install(self.script, '--team-root', root, '--port', 'abc')
        self.assertEqual((r.returncode, r.stderr.strip()), (1, '✗ --port 要是 1024～65535 的端口'))
        r = b.install(self.script, '--team-root', root, '--web-port', '80')
        self.assertEqual((r.returncode, r.stderr.strip()), (1, '✗ --web-port 要是 1024～65535 的端口'))
        r = b.install(self.script, '--dir', 'relative')
        self.assertIn('绝对路径', r.stderr)
        for d in ('/', '//', '/.', '/opt/../etc', '/a//b', '/a b'):   # 会落到 / 或者别处：卸载时删的是目录里的 data 这些
            r = b.install(self.script, '--dir', d)
            self.assertEqual(r.returncode, 1, d)
            self.assertIn('要是 / 下面的一个目录', r.stderr, d)
        for d in ('/home/me/hub', '/home', '/etc/cca-hub', '/usr/local/hub/', '/tmp/hub', '/run/hub'):   # systemd 的服务看不到、写不了
            r = b.install(self.script, '--team-root', root, '--dir', d)
            self.assertEqual(r.returncode, 1, d)
            self.assertIn('systemd 方式的服务看不到', r.stderr, d)
        r = b.install(self.script, '--bogus')
        self.assertEqual(r.returncode, 2)
        self.assertIn('用法', r.stderr)
        for args, want in (((), '要给 --team-root'), (('--dir', '/opt/new'), '/opt/new 里没有装过 Team 的同步服务器'),
                           (('--team-root', root, '--reset'), '没有 --reset'), (('--team-root', 'ssh-ed25519 AAAA'), '根公钥不对')):
            r = b.install(self.script, *args)
            self.assertEqual(r.returncode, 1, args)
            self.assertIn(want, r.stderr, args)
        (b.d / 'uid').write_text('1000')
        r = b.install(self.script, '--team-root', root)
        self.assertEqual(r.returncode, 1)
        self.assertIn('要用 root 运行', r.stderr)
        self.assertEqual(sorted(p.name for p in b.root.iterdir()), ['etc', 'run'], '什么都没动')

    def test_systemd(self):
        """systemd + python3：装好、打出管理员接上它的那一行（地址是内网的：网页端不开，说一声）；目录、权限、unit；再运行是更新；换端口
        照记下的地址再打一次那一行；--uninstall"""
        b = self.box()
        a, tid, root, svc = self.team()
        d = b.root / 'opt' / svc
        port = free_port()
        r = self.install(b, '--team-root', root, '--host', '127.0.0.1', '--port', str(port))
        self.assertIn('✓ Team 的同步服务器在运行（/opt/%s，systemd + python' % svc, r.stdout)
        self.assertIn('cca team attach Team https://127.0.0.1:%d %s' % (port, pin(d)), r.stdout)
        self.assertIn('手机的网页端没开：127.0.0.1 是内网地址', r.stdout)
        self.assertIn('要放行 TCP %d、%d（远程会话的中转）。' % (port, port + 1), r.stdout)
        e = b.envfile(d)
        self.assertEqual((e['CCA_HUB_MODE'], e['CCA_HUB_PORT'], e['CCA_TEAM_ROOT'], e['CCA_HUB_RELAY'], e['CCA_HUB_HOST'], e['CCA_HUB_VERSION']),
                         ('systemd', str(port), root.split()[1], str(port + 1), '127.0.0.1', '9.9.9'))
        self.assertTrue(e['CCA_HUB_WEB'].isdigit() and e['CCA_HUB_WEB'] not in (str(port), str(port + 1)), '网页端的端口照样选好、记着（下次再看）')
        self.assertFalse({'CCA_HUB_IMAGE', 'CCA_HUB_WEB_URL'} & set(e))
        modes = {p: oct((d / p).stat().st_mode & 0o777)
                 for p in ('.', '.env', '.cca-hub', 'app', 'app/web', 'app/web/index.html', 'certs', 'certs/key.pem', 'certs/cert.pem', 'data')}
        self.assertEqual(modes, {'.': '0o755', '.env': '0o600', '.cca-hub': '0o644', 'app': '0o755', 'app/web': '0o755',
                                 'app/web/index.html': '0o644', 'certs': '0o700', 'certs/key.pem': '0o600', 'certs/cert.pem': '0o644',
                                 'data': '0o700'})
        self.assertEqual(sorted(p.name for p in d.iterdir()), ['.cca-hub', '.env', 'app', 'certs', 'data'], '证书助手不在这里（共用的那一个）')
        self.assertFalse((d / '.env.prev').exists(), '装好了：不留原来的 .env')
        unit = (b.root / UNITS / (svc + '.service')).read_text()
        for want in ('User=%s\n' % svc, 'EnvironmentFile=/opt/%s/.env\n' % svc, 'NoNewPrivileges=yes', 'ProtectSystem=full', 'TasksMax=64\n',
                     '--mode team --listen 0.0.0.0:${CCA_HUB_PORT} --cert /opt/%s/certs/cert.pem' % svc,
                     '--relay-listen 0.0.0.0:%d --relay-port %d\n' % (port + 1, port + 1)):
            self.assertIn(want, unit)
        self.assertRegex(unit, r'ExecStart=/\S+python3\S* -I /opt/%s/app/cca-presence.py ' % svc)
        self.assertNotIn('--web', unit)
        self.assertFalse((b.root / UNITS / (ASVC + '.service')).exists() or (b.root / 'opt' / ASVC).exists(), '网页端没开：没有证书助手')
        self.assertIn('-r -M -d /opt/%s' % svc, b.log('useradd.log'))
        self.assertIn('-R %s %s/certs %s/data\n' % (svc, d, d), b.log('chown.log'))
        self.assertEqual(b.log('systemctl.log').split('\n')[:3], ['daemon-reload', 'enable ' + svc, 'restart ' + svc])
        self.assertEqual((d / 'app' / 'cca-presence.py').read_text(), (ROOT / 'server' / 'cca-presence.py').read_text())
        self.assertEqual(sorted(p.name for p in (d / 'app').iterdir()), ['cca-presence.py', 'web'], '没写 --name：没有 name')
        self.assertEqual(sorted(p.name for p in (d / 'app' / 'web').iterdir()), sorted(p.name for p in WEB.iterdir()))
        for f in WEB.iterdir():
            self.assertEqual((d / 'app' / 'web' / f.name).read_bytes(), f.read_bytes(), f.name)
        self.assertEqual(a.ok('team-attach', tid, 'https://127.0.0.1:%d' % port, pin(d)).split('\n')[0], '127.0.0.1:%d' % port)
        # 再运行（只写目录）：更新，端口、证书没变：不再打接上它的那一行
        r = self.install(b, '--dir', '/opt/' + svc)
        self.assertIn('更新了服务端程序', r.stdout)
        self.assertNotIn('cca team attach', r.stdout)
        self.assertEqual(a.ok('team-sync').split('\t')[2], 'ok')
        # 换端口：照记下的地址打出接上它的那一行（管理员重新接上，各台设备照配置里的新地址换过来）
        p2 = free_port()
        r = self.install(b, '--dir', '/opt/' + svc, '--port', str(p2))
        self.assertIn('cca team attach Team https://127.0.0.1:%d %s' % (p2, pin(d)), r.stdout)
        self.assertEqual(b.envfile(d)['CCA_HUB_PORT'], str(p2))
        # 卸载：停掉、删掉服务器上的文件和用户
        r = self.install(b, '--uninstall', '--dir', '/opt/' + svc)
        self.assertIn('卸载了 Team 的同步服务器', r.stdout)
        self.assertFalse(d.exists() or (b.root / UNITS / (svc + '.service')).exists() or b.running(svc))
        self.assertEqual(list((b.d / 'users').iterdir()), [])
        self.assertIn('disable --now ' + svc, b.log('systemctl.log'))

    def test_web(self):
        """手机的网页端（systemd）：另开一个端口；这台服务器上共用的证书助手（cca-team-acme：自己的系统用户，只给绑 80 端口的权限）照登记的地址
        要来证书、放进这个 Team 的那一份（证书助手的用户、这个 Team 的组，setgid），服务端用它给出页面，标题带上 Team 的名字（--name）；
        再运行照旧；--no-web 去掉登记、停掉证书助手（没有别的 Team 用它了；证书留着）、再运行还是关着；80 端口被别的程序占着时不开（写了
        --web-port 就停下）；证书没要到时说放行 80；卸载最后一个用它的 Team 时连同证书助手一起删掉"""
        b = self.box(any_ip=True)
        a, tid, root, svc = self.team()
        d, ad = b.root / 'opt' / svc, b.root / 'opt' / ASVC
        port, web = two_ports()
        url = 'https://127.0.0.1:%d/' % web
        r = self.install(b, '--team-root', root, '--host', '127.0.0.1', '--port', str(port), '--web-port', str(web), '--name', '甲组')
        self.assertIn("手机的网页端：%s（证书是 Let's Encrypt 签的" % url, r.stdout)
        self.assertIn('要放行 TCP %d、%d（远程会话的中转）、%d 和 80（手机的网页端；80 端口给证书机构验证）' % (port, port + 1, web), r.stdout)
        e = b.envfile(d)
        self.assertEqual((e['CCA_HUB_WEB'], e['CCA_HUB_WEB_URL']), (str(web), url))
        self.assertEqual((d / 'app' / 'name').read_text(), '甲组\n')
        unit = (b.root / UNITS / (svc + '.service')).read_text()
        self.assertIn(' --web-url %s --web-listen 0.0.0.0:%d --web-cert /opt/%s/webcert/%s/fullchain.pem --web-key /opt/%s/webcert/%s/privkey.pem'
                      ' --web-name-file /opt/%s/app/name\n' % (url, web, ASVC, svc, ASVC, svc, svc), unit)
        self.assertIn('TasksMax=512\n', unit, '网页端开着：每条 WebSocket 一个线程')
        acme = (b.root / UNITS / (ASVC + '.service')).read_text()
        for want in ('User=%s\n' % ASVC, 'AmbientCapabilities=CAP_NET_BIND_SERVICE\n', 'CapabilityBoundingSet=CAP_NET_BIND_SERVICE\n',
                     'NoNewPrivileges=yes', '-I /opt/%s/app/cca-acme.py --teams /opt/%s/teams --store /opt/%s/store --out /opt/%s/webcert '
                     '--account /opt/%s/acme --listen 0.0.0.0:80\n' % ((ASVC,) * 5)):
            self.assertIn(want, acme)
        self.assertFalse((b.root / UNITS / (svc + '-acme.service')).exists(), '不再是每个 Team 自己一个')
        self.assertTrue(b.running(ASVC))
        self.assertEqual((ad / 'teams' / svc).read_text(), '127.0.0.1\n', '登记了这个 Team 的地址')
        self.assertEqual((ad / 'app' / 'cca-acme.py').read_text(), (ROOT / 'server' / 'cca-acme.py').read_text())
        modes = {p: oct((ad / p).stat().st_mode & 0o7777) for p in ('.', '.env', 'app', 'teams', 'store', 'acme', 'webcert', 'webcert/' + svc)}
        self.assertEqual(modes, {'.': '0o755', '.env': '0o600', 'app': '0o755', 'teams': '0o755', 'store': '0o700', 'acme': '0o700',
                                 'webcert': '0o711', 'webcert/' + svc: '0o2750'})
        self.assertEqual(b.envfile(ad), {'CCA_HUB_MODE': 'systemd', 'CCA_HUB_VERSION': '9.9.9'})
        gid = (b.d / 'users' / svc).read_text().strip()
        self.assertIn('-R %s:%s %s/webcert/%s\n' % (ASVC, gid, ad, svc), b.log('chown.log'), '这个 Team 的那一份：证书助手的用户、这个 Team 的组')
        self.assertIn('-r -M -d /opt/%s' % ASVC, b.log('useradd.log'))
        self.assertIn('--teams %s/teams --store %s/store --out %s/webcert --account %s/acme' % ((ad,) * 4), b.log('acme.argv'))
        for f in ('fullchain.pem', 'privkey.pem'):
            self.assertEqual((ad / 'webcert' / svc / f).read_bytes(), (ad / 'store' / '127.0.0.1' / f).read_bytes())
        st, body = https(web, '/')
        self.assertEqual(st, 200)
        self.assertIn('<title>cca 远程会话（甲组）</title>', body.decode(), '网页端用证书助手要来的证书给出页面，标题带上 Team 的名字')
        self.assertEqual(health(port).get('web'), url)
        # 再运行：照旧开着；80 端口上听着的是共用的证书助手，不算被占着；没写 --name 照上次的
        (b.d / 'listening').write_text('80\n')
        b.fake('ss', '#!/bin/sh\n' + SS)
        r = self.install(b, '--dir', '/opt/' + svc)
        self.assertIn('手机的网页端：' + url, r.stdout)
        self.assertNotIn('安全组', r.stdout, '端口都没变：不再说要放行')
        self.assertTrue(b.running(ASVC))
        self.assertEqual((d / 'app' / 'name').read_text(), '甲组\n')
        # cca team deploy 没写 --host 时放进 ssh -G 的主机名当兜底：记着地址时照记着的（ssh 走的是内网地址也不拿它盖掉、不关网页端）
        r = self.install(b, '--dir', '/opt/' + svc, env={'CCA_HUB_HOST_HINT': '10.0.0.5'})
        self.assertIn('手机的网页端：' + url, r.stdout)
        self.assertEqual((b.envfile(d)['CCA_HUB_HOST'], b.envfile(d)['CCA_HUB_WEB_URL']), ('127.0.0.1', url))
        # --no-web：去掉登记和这个 Team 的那一份；没有别的 Team 用证书助手了：停掉它、删掉 unit（证书、账户留着，再开时接着用）
        r = self.install(b, '--dir', '/opt/' + svc, '--no-web')
        e = b.envfile(d)
        self.assertEqual((e['CCA_HUB_WEB'], 'CCA_HUB_WEB_URL' in e), ('off', False))
        self.assertFalse((b.root / UNITS / (ASVC + '.service')).exists() or b.running(ASVC))
        self.assertFalse((ad / 'teams' / svc).exists() or (ad / 'webcert' / svc).exists())
        self.assertTrue((ad / 'store' / '127.0.0.1' / 'fullchain.pem').exists(), '证书留着')
        self.assertNotIn('--web', (b.root / UNITS / (svc + '.service')).read_text())
        self.assertNotIn('web', health(port))
        self.assertNotIn('手机的网页端', r.stdout, '自己关的：不说')
        r = self.install(b, '--dir', '/opt/' + svc)
        self.assertEqual(b.envfile(d)['CCA_HUB_WEB'], 'off')
        # 再打开，80 端口却被别的程序占着：写了 --web-port 就停下，什么都不动
        r = b.install(self.script, '--dir', '/opt/' + svc, '--web-port', str(web))
        self.assertEqual(r.returncode, 1)
        self.assertIn('80 端口被别的程序占着：证书助手要用它', r.stderr)
        self.assertEqual(b.envfile(d)['CCA_HUB_WEB'], 'off')
        # 80 端口腾出来了，证书却没要到（安全组没放行 80；存着的那张也没了）：照样开着，说怎么办
        (b.d / 'listening').write_text('')
        shutil.rmtree(ad / 'store')
        b.flag('acme_fail')
        r = self.install(b, '--dir', '/opt/' + svc, '--web-port', str(web))
        self.assertIn('手机的网页端（%s）的证书还没拿到：证书机构要从公网连这台服务器的 80 端口' % url, r.stdout)
        self.assertIn('journalctl -u %s' % ASVC, r.stdout)
        self.assertTrue(b.running(ASVC), '证书助手接着试')
        # 卸载最后一个用证书助手的 Team：连同它（目录、unit、系统用户）一起删掉
        r = self.install(b, '--uninstall', '--dir', '/opt/' + svc)
        self.assertIn('共用的证书助手（/opt/%s，连同证书、账户）也删掉了' % ASVC, r.stdout)
        self.assertFalse(d.exists() or ad.exists() or (b.root / UNITS / (ASVC + '.service')).exists() or b.running(ASVC) or b.running(svc))
        self.assertIn('disable --now %s' % ASVC, b.log('systemctl.log'))
        self.assertEqual(list((b.d / 'users').iterdir()), [])

    def test_web_two_teams(self):
        """一台服务器上两个 Team 都开网页端：第二个在共用的证书助手那里登记（80 端口上是它，不算被占着），同一个地址只要一张证书，
        两个 Team 各拿到一份（各是各的组）、各用各的端口和标题；卸载一个，另一个照旧（证书助手还在）；再卸载最后一个，证书助手一起删掉"""
        b = self.box(any_ip=True)
        a1, a2 = self.admin('a'), self.admin('b')
        (tid1, root1, svc1), (tid2, root2, svc2) = self.new_team(a1, '甲组'), self.new_team(a2, '乙组')
        ad = b.root / 'opt' / ASVC
        (p1, w1), (p2, w2) = apart(2)
        self.install(b, '--team-root', root1, '--host', '127.0.0.1', '--port', str(p1), '--web-port', str(w1), '--name', '甲组')
        (b.d / 'listening').write_text('80\n')
        b.fake('ss', '#!/bin/sh\n' + SS)
        r = self.install(b, '--team-root', root2, '--host', '127.0.0.1', '--port', str(p2), '--web-port', str(w2), '--name', '乙组')
        self.assertIn('手机的网页端：https://127.0.0.1:%d/（证书是' % w2, r.stdout, '80 端口上是共用的证书助手：第二个 Team 照样开得了')
        self.assertEqual(sorted(p.name for p in (ad / 'teams').iterdir()), sorted([svc1, svc2]))
        self.assertEqual(b.log('acme.issued'), '127.0.0.1\n', '同一个地址只要一张')
        for f in ('fullchain.pem', 'privkey.pem'):
            self.assertEqual((ad / 'webcert' / svc1 / f).read_bytes(), (ad / 'webcert' / svc2 / f).read_bytes())
        g1, g2 = ((b.d / 'users' / s).read_text().strip() for s in (svc1, svc2))
        self.assertNotEqual(g1, g2)
        for s, g in ((svc1, g1), (svc2, g2)):
            self.assertIn('-R %s:%s %s/webcert/%s\n' % (ASVC, g, ad, s), b.log('chown.log'), '各是各的组：别的 Team 读不了它的私钥')
        for w, n in ((w1, '甲组'), (w2, '乙组')):
            st, body = https(w, '/')
            self.assertEqual(st, 200)
            self.assertIn('<title>cca 远程会话（%s）</title>' % n, body.decode())
        self.install(b, '--uninstall', '--dir', '/opt/' + svc1)
        self.assertTrue(b.running(ASVC) and b.running(svc2) and (ad / 'teams' / svc2).exists())
        self.assertFalse((ad / 'teams' / svc1).exists() or (ad / 'webcert' / svc1).exists())
        self.assertEqual(https(w2, '/')[0], 200)
        r = self.install(b, '--uninstall', '--dir', '/opt/' + svc2)
        self.assertIn('也删掉了', r.stdout)
        self.assertFalse(ad.exists() or b.running(ASVC))

    def test_web_old_helper(self):
        """以前的安装脚本装的：每个 Team 自己一个证书助手（<服务名>-acme，占着 80 端口，证书在这个 Team 的 webcert/ 里）。另一个 Team 这时
        开不了网页端（说是哪个占着、先更新它；写了 --web-port 就停下）；用这一版更新那个 Team：停掉它自己的，证书、账户搬给共用的证书助手
        （接着用那张，不再要），再装另一个就开得了"""
        b = self.box(any_ip=True)
        a1, a2 = self.admin('a'), self.admin('b')
        (tid1, root1, svc1), (tid2, root2, svc2) = self.new_team(a1, '甲组'), self.new_team(a2, '乙组')
        d1, ad = b.root / 'opt' / svc1, b.root / 'opt' / ASVC
        (p1, w1), (p2, w2) = apart(2)
        # 甲组：先照这一版装（网页端关着），再摆成以前的样子——它自己的证书助手的 unit、webcert/、acme/，起着
        self.install(b, '--team-root', root1, '--host', '127.0.0.1', '--port', str(p1), '--no-web')
        env = (d1 / '.env').read_text().replace('CCA_HUB_WEB=off\n', 'CCA_HUB_WEB=%d\nCCA_HUB_WEB_URL=https://127.0.0.1:%d/\n' % (w1, w1))
        (d1 / '.env').write_text(env)
        for x in ('webcert', 'acme'):
            (d1 / x).mkdir(mode=0o700)
        (d1 / 'acme' / 'account.key').write_text('以前的账户的私钥\n')
        (b.root / UNITS / (svc1 + '-acme.service')).write_text(
            '[Service]\nUser=%s\nEnvironmentFile=/opt/%s/.env\nExecStart=/usr/bin/python3 -I /opt/%s/app/cca-acme.py --id ${CCA_HUB_HOST} '
            '--out /opt/%s/webcert --account /opt/%s/acme --listen 0.0.0.0:80\n' % (svc1, svc1, svc1, svc1, svc1))
        subprocess.run([str(b.bin / 'systemctl'), 'restart', svc1 + '-acme'], env=b.env, check=True)
        wait_for('以前的证书助手要来证书', lambda: (d1 / 'webcert' / 'fullchain.pem').exists())
        old = {f: (d1 / 'webcert' / f).read_bytes() for f in ('fullchain.pem', 'privkey.pem')}
        (b.d / 'listening').write_text('80\n')
        b.fake('ss', '#!/bin/sh\n' + SS)
        # 乙组：80 端口被甲组以前的证书助手占着
        r = self.install(b, '--team-root', root2, '--host', '127.0.0.1', '--port', str(p2))
        self.assertIn('手机的网页端没开：80 端口被 %s-acme 占着' % svc1, r.stdout)
        r = b.install(self.script, '--dir', '/opt/' + svc2, '--web-port', str(w2))
        self.assertEqual(r.returncode, 1)
        self.assertIn('80 端口被 %s-acme 占着' % svc1, r.stderr)
        self.assertFalse(ad.exists(), '还没有共用的证书助手')
        # 用这一版更新甲组：它自己的证书助手停掉，证书、账户搬给共用的证书助手，接着用那张
        r = self.install(b, '--dir', '/opt/' + svc1)
        self.assertIn('手机的网页端：https://127.0.0.1:%d/（证书是' % w1, r.stdout)
        self.assertIn('disable --now %s-acme' % svc1, b.log('systemctl.log'))
        self.assertFalse(b.running(svc1 + '-acme') or (b.root / UNITS / (svc1 + '-acme.service')).exists())
        self.assertFalse((d1 / 'webcert').exists() or (d1 / 'acme').exists(), '以前的目录搬空、删掉了')
        self.assertEqual((ad / 'acme' / 'account.key').read_text(), '以前的账户的私钥\n', '账户搬过来了')
        for f, raw in old.items():
            self.assertEqual((ad / 'webcert' / svc1 / f).read_bytes(), raw, f + '：接着用原来的那张')
        self.assertEqual(b.log('acme.issued'), '127.0.0.1\n', '没有再要一张')
        self.assertTrue(b.running(ASVC))
        # 再装乙组：开得了
        r = self.install(b, '--dir', '/opt/' + svc2, '--web-port', str(w2))
        self.assertIn('手机的网页端：https://127.0.0.1:%d/（证书是' % w2, r.stdout)
        self.assertEqual((ad / 'webcert' / svc2 / 'fullchain.pem').read_bytes(), old['fullchain.pem'])
        self.assertEqual(b.log('acme.issued'), '127.0.0.1\n')

    def test_web_skipped(self):
        """网页端这一次开不了：80 端口被别的程序占着、地址是内网的、认不出地址、它的端口被占着——照样装好，说一声，选的端口记着（下次再看）；
        不开中转就没有网页端（写了 --web-port 是写错了）"""
        b = self.box(any_ip=True)
        a, tid, root, svc = self.team()
        d = b.root / 'opt' / svc
        (b.d / 'listening').write_text('80\n')
        b.fake('ss', '#!/bin/sh\n' + SS)
        port = free_port()
        r = self.install(b, '--team-root', root, '--host', '127.0.0.1', '--port', str(port))
        self.assertIn('手机的网页端没开：80 端口被别的程序占着', r.stdout)
        e = b.envfile(d)
        self.assertTrue(e['CCA_HUB_WEB'].isdigit() and 'CCA_HUB_WEB_URL' not in e)
        self.assertFalse((b.root / UNITS / (ASVC + '.service')).exists() or (b.root / 'opt' / ASVC).exists())
        r = b.install(self.script, '--dir', '/opt/' + svc, '--web-port', str(free_port()))
        self.assertEqual(r.returncode, 1)
        self.assertIn('80 端口被别的程序占着', r.stderr)
        # 80 端口腾出来了：再运行（没写什么）就开上，用记着的端口
        (b.d / 'listening').write_text('')
        r = self.install(b, '--dir', '/opt/' + svc)
        self.assertIn('手机的网页端：https://127.0.0.1:%s/' % e['CCA_HUB_WEB'], r.stdout)
        self.assertIn('要放行 TCP %d、%d（远程会话的中转）、%s 和 80' % (port, port + 1, e['CCA_HUB_WEB']), r.stdout, '网页端这次才开：说要放行')
        # 网页端的端口被别的程序占着（没写 --web-port）：这一次不开
        b.stop(svc)
        (b.d / 'listening').write_text('%s\n' % e['CCA_HUB_WEB'])
        r = self.install(b, '--dir', '/opt/' + svc)
        self.assertIn('手机的网页端没开：端口 %s 被别的程序占着' % e['CCA_HUB_WEB'], r.stdout)
        (b.d / 'listening').write_text('')
        # 不开中转：没有网页端（不说），写了 --web-port 是写错了
        r = b.install(self.script, '--dir', '/opt/' + svc, '--no-relay', '--web-port', str(free_port()))
        self.assertEqual(r.returncode, 1)
        self.assertIn('不能和 --no-relay 一起', r.stderr)
        r = self.install(b, '--dir', '/opt/' + svc, '--no-relay')
        self.assertNotIn('网页端', r.stdout)
        self.assertEqual(b.envfile(d)['CCA_HUB_RELAY'], 'off')
        self.assertFalse(b.running(ASVC) or (b.root / 'opt' / ASVC / 'teams' / svc).exists(), '没有 Team 用证书助手了：停掉')
        # 认不出地址（以前的安装脚本没记 CCA_HUB_HOST，机器上又看不到公网 IP）：网页端不开，说加 --host
        r = self.install(b, '--dir', '/opt/' + svc, '--relay-port', str(free_port()))
        env = (d / '.env').read_text()
        (d / '.env').write_text(''.join(l + '\n' for l in env.splitlines() if not l.startswith('CCA_HUB_HOST=')))
        (b.d / 'ip').write_text('10.9.0.5')
        r = self.install(b, '--dir', '/opt/' + svc)
        self.assertIn('手机的网页端没开：认不出这台服务器的公网地址', r.stdout)
        # 没记着地址时才用 cca team deploy 放进的兜底（ssh -G 的主机名），记下来
        r = self.install(b, '--dir', '/opt/' + svc, env={'CCA_HUB_HOST_HINT': '127.0.0.1'})
        self.assertEqual(b.envfile(d)['CCA_HUB_HOST'], '127.0.0.1')
        self.assertIn('手机的网页端：https://127.0.0.1:%s/' % b.envfile(d)['CCA_HUB_WEB'], r.stdout)
        # 内网地址（不是测试里的 any_ip）：不开
        c = self.box()
        r = self.install(c, '--team-root', root, '--host', '10.0.0.5', '--port', str(free_port()))
        self.assertIn('手机的网页端没开：10.0.0.5 是内网地址', r.stdout)

    def test_docker(self):
        """Docker compose：用第一个拉得到的镜像，容器里的用户 10001，项目名是服务名；网页端开着时共用的证书助手也是 Docker 的（自己的
        compose 项目 cca-team-acme，80 端口），这个 Team 只读挂着它放给自己的那一份；改用 systemd 时这个 Team 的容器停掉，证书助手照旧
        （Docker 还能用）；拉不到镜像时自动改用 systemd，指定了 --docker 就停下"""
        b = self.box(docker=True, any_ip=True)
        a, tid, root, svc = self.team()
        d = b.root / 'opt' / svc
        port, web = two_ports()
        r = b.install(self.script, '--team-root', root, '--host', '127.0.0.1', '--port', str(port), '--web-port', str(web), shell='/bin/sh')
        self.assertEqual((r.returncode, r.stderr), (0, ''), r.stdout)
        e = b.envfile(d)
        self.assertEqual((e['CCA_HUB_MODE'], e['CCA_HUB_IMAGE']), ('docker', IMAGES[0]))
        self.assertIn('Docker compose，镜像 ' + IMAGES[0], r.stdout)
        y = (d / 'compose.yaml').read_text()
        for want in ('container_name: %s\n' % svc, 'user: "10001:10001"', 'read_only: true', 'cap_drop: [ALL]', '- ./data:/data:z',
                     '- "${CCA_HUB_WEB:?}:18447"', '- /opt/%s/webcert/%s:/webcert:ro,z\n' % (ASVC, svc), 'pids_limit: 512',
                     '"--web-url", "${CCA_HUB_WEB_URL:?}"', '"--web-key", "/webcert/privkey.pem"]', 'CCA_TEAM_ROOT: ${CCA_TEAM_ROOT:?}'):
            self.assertIn(want, y)
        self.assertNotIn('-acme:', y, '证书助手不在这个 Team 的项目里')
        self.assertNotIn('--web-name-file', y, '没写 --name')
        self.assertNotIn('@', y, '记号都换掉了')
        ad = b.root / 'opt' / ASVC
        ay = (ad / 'compose.yaml').read_text()
        for want in ('container_name: %s\n' % ASVC, 'user: "10001:10001"', 'read_only: true', 'cap_drop: [ALL]', '- "80:18080"',
                     '- ./teams:/teams:ro,z', '- ./store:/store:z', '- ./webcert:/webcert:z', '- ./acme:/acme:z',
                     '"--teams", "/teams", "--store", "/store", "--out", "/webcert", "--account", "/acme", "--listen", "0.0.0.0:18080"'):
            self.assertIn(want, ay)
        self.assertEqual(b.envfile(ad), {'CCA_HUB_MODE': 'docker', 'CCA_HUB_IMAGE': IMAGES[0], 'CCA_HUB_VERSION': '9.9.9'})
        chown = b.log('chown.log')
        for want in ('-R 10001:10001 %s/certs %s/data\n' % (d, d), '-R 10001 %s/store %s/acme %s/webcert\n' % (ad, ad, ad),
                     '-R 10001:10001 %s/webcert/%s\n' % (ad, svc)):
            self.assertIn(want, chown)
        log = b.log('docker.log')
        self.assertIn('compose -p %s -f compose.yaml up -d --force-recreate --remove-orphans' % svc, log)
        self.assertIn('compose -p %s -f compose.yaml up -d --force-recreate --remove-orphans' % ASVC, log)
        self.assertFalse((b.root / UNITS / (svc + '.service')).exists() or (b.root / UNITS / (ASVC + '.service')).exists())
        self.assertEqual(b.log('useradd.log'), '')
        self.assertTrue(b.running(svc) and b.running(ASVC))
        self.assertEqual(https(web, '/')[0], 200)
        self.assertIn("手机的网页端：https://127.0.0.1:%d/（证书是 Let's Encrypt 签的" % web, r.stdout)
        self.assertIn('docker logs', self.script.read_text(), '证书没要到时说看哪个容器的日志')
        a.ok('team-attach', tid, 'https://127.0.0.1:%d' % port, pin(d))
        # 改用 systemd（--systemd）：这个 Team 的容器停掉，数据照旧；共用的证书助手照旧是 Docker 的（还能用），放给它的那一份换成它的组
        r = self.install(b, '--dir', '/opt/' + svc, '--systemd')
        self.assertIn('compose -p %s -f compose.yaml down' % svc, b.log('docker.log'))
        self.assertFalse((d / 'compose.yaml').exists())
        self.assertEqual(b.envfile(d)['CCA_HUB_MODE'], 'systemd')
        self.assertTrue((ad / 'compose.yaml').exists() and b.running(ASVC) and not (b.root / UNITS / (ASVC + '.service')).exists())
        self.assertIn('-R 10001:%s %s/webcert/%s\n' % ((b.d / 'users' / svc).read_text().strip(), ad, svc), b.log('chown.log'))
        self.assertIn("手机的网页端：https://127.0.0.1:%d/（证书是" % web, r.stdout)
        self.assertEqual(a.ok('team-sync').split('\t')[2], 'ok', '换了运行方式，照旧连得上')
        r = self.install(b, '--dir', '/opt/' + svc)
        self.assertIn('systemd + python', r.stdout, '没指定：照上次的')
        # 拉不到镜像
        c = self.box(docker=True)
        c.flag('no_pull')
        r = self.install(c, '--team-root', root, '--host', '127.0.0.1', '--port', str(free_port()))
        self.assertIn('改用 systemd', r.stdout)
        self.assertEqual(c.envfile(c.root / 'opt' / svc)['CCA_HUB_MODE'], 'systemd')
        f = self.box(docker=True)
        f.flag('no_pull')
        r = f.install(self.script, '--team-root', root, '--docker', '--host', '127.0.0.1', '--port', str(free_port()))
        self.assertEqual(r.returncode, 1)
        self.assertIn('拉不到 python:3.13-slim 镜像', r.stderr)
        self.assertFalse((f.root / 'opt' / svc).exists(), '没拉到镜像：什么都还没动')

    def test_compose(self):
        """只用 compose 插件：只有老的 docker-compose 1.x 时改用 systemd（--docker 时停下）；compose up 起不来时打出它说的、
        改用 systemd（--docker 时停下，第一次装的 .env 不留）；Docker 的卸载"""
        a, tid, root, svc = self.team()
        b = self.box(docker=True)
        b.flag('no_compose')
        self.install(b, '--team-root', root, '--host', '127.0.0.1', '--port', str(free_port()))
        self.assertEqual(b.envfile(b.root / 'opt' / svc)['CCA_HUB_MODE'], 'systemd')
        r = b.install(self.script, '--dir', '/opt/' + svc, '--docker')
        self.assertEqual(r.returncode, 1)
        self.assertIn('带 compose 插件', r.stderr)
        self.assertEqual(b.log('docker-compose.log'), '', '不用老的 docker-compose')

        c = self.box(docker=True)
        c.flag('compose_fail')
        d = c.root / 'opt' / svc
        r = c.install(self.script, '--team-root', root, '--host', '127.0.0.1', '--port', str(free_port()))
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn('    Error response from daemon', r.stderr, 'compose 说的打出来（缩进）')
        self.assertIn('Docker compose 起不来（上面是它说的）：改用 systemd', r.stdout)
        self.assertEqual(c.envfile(d)['CCA_HUB_MODE'], 'systemd')
        self.assertNotIn('CCA_HUB_IMAGE', c.envfile(d))
        self.assertFalse((d / 'compose.yaml').exists())
        self.assertIn('compose -p %s -f compose.yaml down' % svc, c.log('docker.log'))
        self.assertTrue(c.running(svc))

        e = self.box(docker=True)
        e.flag('compose_fail')
        d = e.root / 'opt' / svc
        r = e.install(self.script, '--team-root', root, '--docker', '--host', '127.0.0.1', '--port', str(free_port()))
        self.assertEqual(r.returncode, 1)
        self.assertIn('Error response from daemon', r.stderr)
        self.assertIn('docker compose up 没成', r.stderr)
        self.assertFalse((d / '.env').exists(), '第一次装没成：.env 不留（下一次还是第一次装）')
        e.flag('compose_fail', False)
        self.install(e, '--team-root', root, '--host', '127.0.0.1', '--port', str(free_port()))
        self.assertEqual(e.envfile(d)['CCA_HUB_MODE'], 'docker')
        self.install(e, '--uninstall', '--dir', '/opt/' + svc)
        self.assertFalse(d.exists() or e.running(svc))

    def test_dir(self):
        """--dir：别人的目录（里面有东西）不碰，卸载也不碰；装在自己的目录（末尾的 / 去掉）；同一个 Team 一台服务器只装一个"""
        b = self.box()
        a, tid, root, svc = self.team()
        precious = b.root / 'srv' / 'data' / 'precious'
        precious.parent.mkdir(parents=True)
        precious.write_text('别人的数据')
        r = b.install(self.script, '--team-root', root, '--dir', '/srv', '--host', '127.0.0.1')
        self.assertEqual(r.returncode, 1)
        self.assertIn('/srv 里已经有别的东西', r.stderr)
        r = b.install(self.script, '--uninstall', '--dir', '/srv')
        self.assertEqual(r.returncode, 1)
        self.assertIn('/srv 里没有装过 Team 的同步服务器', r.stderr)
        self.assertEqual((precious.read_text(), b.log('chown.log')), ('别人的数据', ''))
        port = free_port()
        self.install(b, '--team-root', root, '--dir', '/srv/team/', '--host', '127.0.0.1', '--port', str(port))
        hub = b.root / 'srv' / 'team'
        self.assertTrue((hub / '.cca-hub').is_file())
        self.assertIn('EnvironmentFile=/srv/team/.env\n', (b.root / UNITS / (svc + '.service')).read_text())
        a.ok('team-attach', tid, 'https://127.0.0.1:%d' % port, pin(hub))
        r = b.install(self.script, '--team-root', root, '--host', '127.0.0.1')   # 默认的 /opt/<服务名>：这个 Team 已经装在 /srv/team 了
        self.assertEqual(r.returncode, 1)
        self.assertIn('已经在 /srv/team 装了这个 Team 的同步服务器', r.stderr)
        self.assertFalse((b.root / 'opt' / svc).exists())
        r = self.install(b, '--dir', '/srv/team')
        self.assertIn('更新了服务端程序', r.stdout)
        self.assertEqual(a.ok('team-sync').split('\t')[2], 'ok')
        self.install(b, '--uninstall', '--dir', '/srv/team')
        self.assertFalse(hub.exists())
        self.assertEqual(precious.read_text(), '别人的数据', '旁边的不动')

    def test_failed_update_rolls_back(self):
        """更新时起不来（.env 已经换了）、被打断：.env 换回原来的，再运行照旧是更新，管理员照样连得上"""
        b = self.box()
        a, tid, root, svc = self.team()
        d = b.root / 'opt' / svc
        port = free_port()
        self.install(b, '--team-root', root, '--host', '127.0.0.1', '--port', str(port))
        a.ok('team-attach', tid, 'https://127.0.0.1:%d' % port, pin(d))
        before = (d / '.env').read_text()
        b.flag('start_fail')
        r = b.install(self.script, '--dir', '/opt/' + svc, '--port', str(free_port()))
        self.assertEqual(r.returncode, 1)
        self.assertIn('systemd 起不来', r.stderr)
        self.assertEqual((d / '.env').read_text(), before, '.env 换回了原来的')
        self.assertFalse((d / '.env.prev').exists())
        b.flag('start_fail', False)
        # 被打断（Ctrl-C、SSH 断了）：在起服务的时候
        b.flag('start_hang')
        restarts = lambda: b.log('systemctl.log').split('\n').count('restart ' + svc)
        n = restarts()   # 上面起不来的那一次最后也是 restart：要等这一次的
        p = subprocess.Popen([SH, str(self.script), '--dir', '/opt/' + svc, '--port', str(free_port())], env=b.env, stdin=subprocess.DEVNULL,
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
        try:
            wait_for('起服务', lambda: restarts() > n and (d / '.env.prev').exists())
            self.assertNotEqual((d / '.env').read_text(), before)
            os.killpg(p.pid, signal.SIGTERM)
            p.communicate(timeout=30)
        finally:
            if p.poll() is None:
                os.killpg(p.pid, signal.SIGKILL)
                p.communicate()
        self.assertEqual(p.returncode, 1)
        self.assertEqual((d / '.env').read_text(), before, '被打断：.env 换回了原来的')
        b.flag('start_hang', False)
        r = self.install(b, '--dir', '/opt/' + svc)
        self.assertIn('更新了服务端程序', r.stdout)
        self.assertEqual(b.envfile(d)['CCA_HUB_PORT'], str(port))
        wait_for('重启后照样连得上', lambda: a.run('team-sync').stdout.split('\t')[2:3] == ['ok'])

    def test_needs(self):
        """缺东西时停下、说怎么办，什么都不动：没有 Docker 也没有 systemd + python3、Python 太旧、端口被占、认不出公网地址；
        机器上看得到的公网 IP 照用（网页端也开）"""
        a, tid, root, svc = self.team()
        b = self.box(systemd=False)
        r = b.install(self.script, '--team-root', root)
        self.assertEqual(r.returncode, 1)
        self.assertIn('两样都没有', r.stderr)
        c = self.box()
        c.fake('python3', '#!/bin/sh\nexit 1\n')   # 3.6 以前的、没有 ssl 的
        r = c.install(self.script, '--team-root', root, '--systemd')
        self.assertIn('要 systemd 和 python3（3.6 起）', r.stderr)
        d = self.box()
        port = free_port()
        (d.d / 'listening').write_text('%d\n' % port)
        d.fake('ss', '#!/bin/sh\n' + SS)
        r = d.install(self.script, '--team-root', root, '--host', '127.0.0.1', '--port', str(port))
        self.assertIn('端口 %d 被别的程序占着' % port, r.stderr)
        # 公网地址：机器上看得到的才认；内网的（云服务器的公网 IP 常常是 NAT 进来的）又没有终端可问时要 --host
        (d.d / 'ip').write_text('10.9.0.5')
        r = d.install(self.script, '--team-root', root, '--port', str(free_port()))
        self.assertIn('加上 --host', r.stderr)
        for x in (b, c, d):
            self.assertFalse((x.root / 'opt' / svc).exists())
        (d.d / 'ip').write_text('203.0.113.7')
        p = free_port()
        r = self.install(d, '--team-root', root, '--port', str(p))
        self.assertIn('cca team attach Team https://203.0.113.7:%d ' % p, r.stdout)
        self.assertIn('手机的网页端：https://203.0.113.7:', r.stdout, '公网地址：网页端开上')
        self.assertEqual(d.envfile(d.root / 'opt' / svc)['CCA_HUB_HOST'], '203.0.113.7')

    def test_old_own_server(self):
        """3.5.3 以前的自己的同步服务器（/opt/cca-hub，服务名 cca-hub）：只能卸载；装 Team 的不往那个目录里装"""
        b = self.box()
        a, tid, root, svc = self.team()
        old = b.dir
        (old / 'data').mkdir(parents=True)
        (old / 'data' / 'state.json').write_text('{}')
        (old / '.env').write_text('CCA_HUB_MODE=systemd\nCCA_HUB_PORT=18445\nCCA_HUB_ADMIT_HASH=%s\n' % ('0' * 64))
        (old / '.cca-hub').write_text('# cca 的同步服务器（自己的同步服务器）装在这个目录\n# svc=cca-hub\n')
        unit = b.root / UNITS / 'cca-hub.service'
        unit.write_text('[Service]\nUser=cca-hub\nEnvironmentFile=/opt/cca-hub/.env\nExecStart=/usr/bin/python3 -I /opt/cca-hub/app/cca-presence.py\n')
        (b.d / 'users' / 'cca-hub').touch()
        for args, want in ((('--dir', '/opt/cca-hub'), '以前的自己的同步服务器（cca hub，3.5.3 起没有了'),
                           (('--team-root', root, '--dir', '/opt/cca-hub'), '装着的是以前的自己的同步服务器（cca hub）'),
                           (('--uninstall', '--team-root', root, '--dir', '/opt/cca-hub'), '不是要卸载的这一个')):
            r = b.install(self.script, *args)
            self.assertEqual(r.returncode, 1, args)
            self.assertIn(want, r.stderr, args)
        self.assertTrue((old / 'data' / 'state.json').exists() and unit.exists())
        r = self.install(b, '--uninstall', '--dir', '/opt/cca-hub')
        self.assertIn('卸载了以前的自己的同步服务器', r.stdout)
        self.assertFalse(old.exists() or unit.exists() or (b.d / 'users' / 'cca-hub').exists())
        self.assertIn('disable --now cca-hub', b.log('systemctl.log'))

    def test_teams(self):
        """两个 Team 各一个服务（服务名、系统用户、目录、端口都是自己的），互不相干；没写端口时从 18445 起找空着的；再运行（只写目录）是
        更新；中转的端口可以换、可以关；装着另一个 Team 的目录不碰；卸载一个，另一个照旧"""
        b = self.box()
        a, c = self.admin('a'), self.admin('c')
        (tid1, root1, svc1), (tid2, root2, svc2) = self.new_team(a, '甲组'), self.new_team(c, '乙组')
        d1, d2 = b.root / 'opt' / svc1, b.root / 'opt' / svc2
        p1 = free_port()
        r = self.install(b, '--team-root', root1, '--host', '127.0.0.1', '--port', str(p1))
        self.assertIn('cca team attach Team https://127.0.0.1:%d %s' % (p1, pin(d1)), r.stdout, '打出管理员接上它的那一行')
        self.assertEqual(health(p1).get('relay'), {'port': p1 + 1})
        self.assertTrue(b.running(svc1) and (b.d / 'users' / svc1).exists() and not b.running(), '服务名、系统用户是这个 Team 的')
        self.assertEqual(a.ok('team-attach', tid1, 'https://127.0.0.1:%d' % p1, pin(d1)).split('\n')[0], '127.0.0.1:%d' % p1)
        # 第二个 Team：只写根公钥后面那一段也行；没写端口，从 18445 起找一个空着的
        k = next(k for k in range(40) if port_free(18445 + k))
        (b.d / 'listening').write_text(''.join('%d\n' % (18445 + i) for i in range(k)))
        b.fake('ss', '#!/bin/sh\n' + SS)
        self.install(b, '--team-root', root2.split()[1], '--host', '127.0.0.1')
        self.assertEqual(b.envfile(d2)['CCA_HUB_PORT'], str(18445 + k), '前面 %d 个端口有程序占着' % k)
        self.assertEqual(b.envfile(d2)['CCA_HUB_RELAY'], str(18446 + k), '各用各的数据端口')
        self.assertTrue(b.running(svc1) and b.running(svc2), '两个都在运行')
        self.assertEqual(c.ok('team-attach', tid2, 'https://127.0.0.1:%d' % (18445 + k), pin(d2)).split('\n')[0], '127.0.0.1:%d' % (18445 + k))
        self.assertEqual(self.code(c, 'team-attach', tid2, 'https://127.0.0.1:%d' % p1, pin(d1)), 'denied', '各是各的：甲组的服务器不认乙组的根密钥')
        # 再运行，只写目录：照这个目录里装的那个 Team 更新（端口、证书、数据都不变）
        before = pin(d1)
        r = self.install(b, '--dir', '/opt/' + svc1)
        self.assertIn('更新了服务端程序', r.stdout)
        self.assertEqual((pin(d1), b.envfile(d1)['CCA_HUB_PORT'], b.running(svc1)), (before, str(p1), True))
        self.assertEqual(a.ok('team-sync').split('\t')[2], 'ok', '管理员照旧连得上')
        self.assertEqual((b.envfile(d1)['CCA_HUB_RELAY'], health(p1).get('relay')), (str(p1 + 1), {'port': p1 + 1}), '中转照上次选的')
        # 不开中转、换一个数据端口：记在 .env 里；写错的不动
        self.install(b, '--dir', '/opt/' + svc1, '--no-relay')
        unit = (b.root / UNITS / (svc1 + '.service')).read_text()
        self.assertEqual((b.envfile(d1)['CCA_HUB_RELAY'], '--relay' in unit, 'relay' in health(p1)), ('off', False, False))
        self.install(b, '--dir', '/opt/' + svc1)
        self.assertEqual(b.envfile(d1)['CCA_HUB_RELAY'], 'off', '关了的再运行还是关着')
        for bad in (str(p1), '80', 'x'):
            r = b.install(self.script, '--dir', '/opt/' + svc1, '--relay-port', bad)
            self.assertNotEqual(r.returncode, 0, bad)
        self.assertEqual(b.envfile(d1)['CCA_HUB_RELAY'], 'off')
        rp = free_port()
        self.install(b, '--dir', '/opt/' + svc1, '--relay-port', str(rp))
        self.assertEqual((b.envfile(d1)['CCA_HUB_RELAY'], health(p1).get('relay')), (str(rp), {'port': rp}))
        # 装着另一个 Team 的目录不碰
        r = b.install(self.script, '--team-root', root2, '--dir', '/opt/' + svc1)
        self.assertNotEqual(r.returncode, 0)
        self.assertIn('另一个 Team 的同步服务器', r.stdout + r.stderr)
        self.assertEqual(b.envfile(d1)['CCA_TEAM_ROOT'], root1.split()[1])
        # 卸载时写错了是哪个（根公钥是乙组的、目录是甲组的）：不动（不然停的是乙组的服务，删的却是甲组的数据）
        r = b.install(self.script, '--uninstall', '--team-root', root2, '--dir', '/opt/' + svc1)
        self.assertNotEqual(r.returncode, 0)
        self.assertIn('不是要卸载的这一个', r.stdout + r.stderr)
        self.assertTrue(b.running(svc1) and b.running(svc2) and (d1 / 'data').exists())
        # .cca-hub 里记着服务名：.env 不见了也认得出是哪个 Team 的——不带根公钥再装不行，卸载照它；写了另一个 Team 的根公钥不碰
        self.assertIn('# svc=%s\n' % svc1, (d1 / '.cca-hub').read_text())
        keep = (d1 / '.env').read_text()
        (d1 / '.env').unlink()
        r = b.install(self.script, '--dir', '/opt/' + svc1)
        self.assertNotEqual(r.returncode, 0)
        self.assertIn('带上 --team-root', r.stdout + r.stderr)
        r = b.install(self.script, '--team-root', root2, '--dir', '/opt/' + svc1)
        self.assertIn('另一个 Team 的同步服务器', r.stdout + r.stderr)
        (d1 / '.env').write_text(keep)
        # 没有 ss、netstat 的服务器上自动挑端口：别的 Team 记着用的端口也跳过
        b.fake('ss', '#!/bin/sh\nexit 127\n')
        e = self.admin('e')
        tid3, root3, svc3 = self.new_team(e, '丙组')
        used = {b.envfile(d1)['CCA_HUB_PORT'], b.envfile(d2)['CCA_HUB_PORT']}
        (b.root / 'opt' / 'cca-team-00000000').mkdir()
        (b.root / 'opt' / 'cca-team-00000000' / '.env').write_text(''.join('CCA_HUB_PORT=%d\n' % (18445 + i) for i in range(k + 1)))
        self.install(b, '--team-root', root3, '--host', '127.0.0.1')
        self.assertNotIn(b.envfile(b.root / 'opt' / svc3)['CCA_HUB_PORT'], used | {str(18445 + i) for i in range(k + 1)})
        self.install(b, '--uninstall', '--dir', '/opt/' + svc3)
        # 卸载第一个：它的服务、目录、系统用户都没了，第二个照旧
        r = self.install(b, '--uninstall', '--dir', '/opt/' + svc1)
        self.assertIn('卸载了 Team 的同步服务器', r.stdout)
        self.assertFalse(b.running(svc1) or d1.exists() or (b.d / 'users' / svc1).exists() or (b.root / UNITS / (svc1 + '.service')).exists())
        self.assertTrue(b.running(svc2) and (b.d / 'users' / svc2).exists() and d2.exists())
        self.assertEqual(c.ok('team-sync').split('\t')[2], 'ok')


class DeployTest(Base):
    """cca team deploy（team-deploy）：经假的 ssh 把同一个脚本交给假服务器，装好了接上"""
    ENV = dict(CCA_VERSION='9.9.9')

    def deploy(self, dev, tid, box, *args, ok=True, **env):
        r = dev.run('team-deploy', tid, 'root@box', *args, HUB_ENTER=str(box.enter), **dict(self.ENV, **env))
        if ok:
            self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        return r

    def test_deploy(self):
        """经 SSH 装好（Docker）、接上，整条路通（发邀请、别的设备加入、同步到配置）；再部署是更新；地址是内网的：网页端不开；托管平台的部署文件"""
        b = self.box(docker=True)
        a, tid, root, svc = self.team()
        d = b.root / 'opt' / svc
        port = free_port()
        self.assertEqual((self.deploy(a, tid, b, '--check').returncode, d.exists()), (0, False), '--check 只看登录得上不')
        r = self.deploy(a, tid, b, '--port', str(port))
        self.assertEqual(kv(r.stdout), {'mode': 'docker', 'state': 'new', 'port': str(port), 'relay': str(port + 1), 'dir': '/opt/' + svc, 'lan': '1',
                                        'host': '127.0.0.1:%d' % port, 'device': 'studio', 'members': '1', 'stored': '1', 'note': '',
                                        'svc': svc, 'web': '', 'web_cert': '', 'web_off': 'lan'})
        self.assertIn('Team 的同步服务器在运行', r.stderr, '服务器那边的进度经 ssh 的 stderr 直接打出来')
        team = json.loads((a.cca / 'teams' / tid / 'team.json').read_text())
        self.assertEqual((team['url'], team['pins']), ('https://127.0.0.1:%d' % port, [pin(d)]))
        self.assertEqual(b.envfile(d)['CCA_HUB_HOST'], '127.0.0.1', '装之前就照 ssh -G 定了地址（网页端的证书要它）')
        y = (d / 'compose.yaml').read_text()
        self.assertTrue(('container_name: %s\n' % svc) in y and '"--mode", "team"' in y and 'CCA_TEAM_ROOT: ${CCA_TEAM_ROOT:?}' in y, y)
        self.assertFalse(ASVC in y or (b.root / 'opt' / ASVC).exists(), '网页端没开：没有证书助手')
        self.assertEqual(set(b.log('compose.projects').split()), {svc}, 'compose 的项目名照服务名（不照目录名）')
        argv = (a.home / 'ssh.argv').read_text()
        self.assertNotIn(str(a.cca), argv)
        self.assertIn('-o ConnectTimeout=15 -G root@box', argv)
        self.assertIn('-o ConnectTimeout=15 -T -o ServerAliveInterval=30 root@box sh -c', argv, '不要伪终端：脚本从 stdin 过去')
        # 发邀请、另一台设备加入：取到了配置
        link = a.ok('team-invite', tid, 'bob-mini').split('\n')[0]
        bob = Device(self.t / ('bob-' + secrets.token_hex(4)), 'bob', {})
        bob.config(lambda c: c.pop('policy'))
        self.assertEqual(bob.ok('team-join', '--name', 'bob-mini', stdin=link).split('\n')[5], 'ok')
        # 再部署：更新，地址照原来接着的，设备不用动
        r = self.deploy(a, tid, b)
        self.assertEqual([kv(r.stdout)[k] for k in ('state', 'port', 'host', 'members')], ['update', str(port), '127.0.0.1:%d' % port, '2'])
        self.assertEqual(bob.ok('team-sync').split('\t')[2], 'ok')
        # 这次 ssh 走的是内网地址（别名、VPN）：服务器上记着的地址不拿它盖掉（网页端的证书、各台设备连的都是记着的那个）
        r = self.deploy(a, tid, b, HUB_SSH_HOST='10.0.0.5')
        self.assertEqual([kv(r.stdout)[k] for k in ('state', 'host')], ['update', '127.0.0.1:%d' % port])
        self.assertEqual(b.envfile(d)['CCA_HUB_HOST'], '127.0.0.1')
        # 托管平台：生成部署文件（服务端程序、README；Railway 另有 Dockerfile），里面只有公开的根公钥（后面那一段）
        key = root.split()[1]
        kvs = lambda text: dict(tok.split('=', 1) for tok in shlex.split(text))   # 值里有换行（steps 是几行）：整个输出一起拆
        out = kvs(a.ok('team-hosted', tid, 'railway', '--dir', str(a.home / 'rw'), CCA_VERSION='9.9.9'))
        self.assertEqual((out['dir'], out['tid'], out['name'], out['svc']), (str(a.home / 'rw'), tid, '甲组', svc))
        self.assertEqual(sorted(f.name for f in (a.home / 'rw').iterdir()), ['Dockerfile', 'README.md', 'cca-presence.py'])
        self.assertIn('ENV CCA_HUB_HOSTED=1 CCA_TEAM_ROOT=%s\n' % key, (a.home / 'rw' / 'Dockerfile').read_text())
        self.assertIn('railway init --name %s' % svc, out['steps'])
        self.assertEqual((a.home / 'rw' / 'cca-presence.py').read_text(), (ROOT / 'server' / 'cca-presence.py').read_text())
        self.assertIn('cca team attach 甲组 https://', (a.home / 'rw' / 'README.md').read_text())
        out = kvs(a.ok('team-hosted', tid, 'sealos'))
        self.assertEqual(out['dir'], str(a.home / (svc + '-sealos')), '没写目录：~/<服务名>-<平台>')
        self.assertIn('环境变量：CCA_TEAM_ROOT=%s' % key, out['steps'])
        self.assertFalse((a.home / (svc + '-sealos') / 'Dockerfile').exists())
        (a.home / 'rw' / 'other.txt').write_text('x')
        self.assertEqual(self.code(a, 'team-hosted', tid, 'railway', '--dir', str(a.home / 'rw')), 'dir')
        self.assertEqual(self.code(a, 'team-hosted', tid, 'heroku'), 'usage')
        self.assertNotIn(str(a.cca), ''.join(f.read_text() for d in ('rw', svc + '-sealos') for f in (a.home / d).iterdir()),
                         '部署文件里没有这台设备上的东西')
        # 写错的：没有这个 Team、Team 的部署没有 --reset、成员不是管理员
        env = dict(HUB_ENTER=str(b.enter), CCA_VERSION='9.9.9')
        self.assertEqual(self.code(a, 'team-deploy', 'nope', 'root@box', **env), 'no_team')
        self.assertEqual(self.code(a, 'team-deploy', tid, 'root@box', '--reset', **env), 'usage')
        self.assertEqual(self.code(bob, 'team-deploy', tid, 'root@box', **env), 'not_admin')

    def test_recorded_host(self):
        """服务器上已经装着这个 Team（在服务器上直接运行安装脚本装的，记着地址），这台设备还没接上它：ssh 走的是内网地址时，
        各台设备连的也是安装脚本定下的（服务器上记着的）那个，和网页端的一致"""
        b = self.box(docker=True)
        a, tid, root, svc = self.team()
        d = b.root / 'opt' / svc
        port = free_port()
        self.install(b, '--team-root', root, '--host', '127.0.0.1', '--port', str(port))
        r = self.deploy(a, tid, b, HUB_SSH_HOST='10.0.0.5')
        self.assertEqual([kv(r.stdout)[k] for k in ('state', 'host', 'members')], ['update', '127.0.0.1:%d' % port, '1'])
        self.assertEqual(json.loads((a.cca / 'teams' / tid / 'team.json').read_text())['url'], 'https://127.0.0.1:%d' % port)
        self.assertEqual(b.envfile(d)['CCA_HUB_HOST'], '127.0.0.1')

    def test_web(self):
        """手机的网页端：装的时候就开上（证书助手要来了证书：web_cert ok；标题带上 Team 的名字：team-deploy 放进 --name）；--no-web 关掉；
        证书没要到是 web_cert wait"""
        b = self.box(docker=True, any_ip=True)
        a, tid, root, svc = self.team()
        d = b.root / 'opt' / svc
        port, web = two_ports()
        url = 'https://127.0.0.1:%d/' % web
        r = self.deploy(a, tid, b, '--port', str(port), '--web-port', str(web))
        o = kv(r.stdout)
        self.assertEqual((o['web'], o['web_cert'], o['web_off']), (url, 'ok', ''))
        self.assertTrue(b.running(ASVC))
        self.assertEqual((d / 'app' / 'name').read_text(), '甲组\n')
        self.assertIn('"--web-name-file", "/app/name"]', (d / 'compose.yaml').read_text())
        st, body = https(web, '/')
        self.assertEqual(st, 200)
        self.assertIn('<title>cca 远程会话（甲组）</title>', body.decode())
        self.assertIn('"short_name": "cca 甲组"', https(web, '/manifest.webmanifest')[1].decode())
        self.assertEqual(health(port).get('web'), url)
        r = self.deploy(a, tid, b)
        self.assertEqual(kv(r.stdout)['web'], url, '再部署：照旧开着')
        r = self.deploy(a, tid, b, '--no-web')
        o = kv(r.stdout)
        self.assertEqual((o['web'], o['web_off']), ('', ''), '自己关的：不说原因')
        self.assertFalse(b.running(ASVC))
        shutil.rmtree(b.root / 'opt' / ASVC / 'store')   # 存着的那张也没了
        b.flag('acme_fail')
        r = self.deploy(a, tid, b, '--web-port', str(web))
        self.assertEqual((kv(r.stdout)['web'], kv(r.stdout)['web_cert']), (url, 'wait'))

    def test_args(self):
        """参数不对：辅助程序先查（usage），不连服务器"""
        b = self.box()
        a, tid, root, svc = self.team()
        for args in (('--port', 'abc'), ('--port', '80'), ('--port', '+2000'), ('--dir', '//'), ('--dir', '/opt/../etc'),
                     ('--dir', '/home/me/hub'), ('--host', 'a b'), ('--ssh-port', '0'), ('--mode', 'podman'), ('--bogus',), ('--port',),
                     ('--web-port', '80'), ('--web-port', '20000', '--port', '20000'), ('--no-web', '--web-port', '20000')):
            r = self.deploy(a, tid, b, *args, ok=False)
            self.assertEqual((r.returncode, r.stdout.split('\t')[0]), (1, 'usage'), args)
        self.assertFalse((a.home / 'ssh.argv').exists(), '不连服务器')

    def test_unreachable_then_again(self):
        """装好了却接不上（安全组没放行）：说要放行哪个端口；放行以后再部署（服务器上是更新），接上；服务器自己的防火墙经 SSH 装时也说"""
        b = self.box()
        a, tid, root, svc = self.team()
        port = free_port()
        (a.home / 'reply').mkdir()
        (a.home / 'reply' / 'team_enroll').write_text('000\n')   # 假 curl：连不上
        r = self.deploy(a, tid, b, '--port', str(port), ok=False)
        code, why = r.stdout.strip().split('\t')
        self.assertEqual(code, 'unreachable')
        self.assertIn('服务器上装好了（127.0.0.1:%d），但没接上' % port, why)
        self.assertIn('放行 TCP %d，放行以后再运行一次' % port, why)
        self.assertFalse(json.loads((a.cca / 'teams' / tid / 'team.json').read_text()).get('url'), '没接上：还是没接着服务器')
        (a.home / 'reply' / 'team_enroll').unlink()
        b.fake('ufw', '#!/bin/sh\n' + UFW)
        r = self.deploy(a, tid, b)
        self.assertEqual([kv(r.stdout)[k] for k in ('mode', 'state', 'port')], ['systemd', 'update', str(port)])
        self.assertIn('这台服务器开着 ufw：运行 ufw allow %d/tcp 放行' % port, r.stderr)

    def test_not_root(self):
        """用普通用户登录：免密码的 sudo 就经它装；sudo 要密码时什么都不运行，说要用 root 登录"""
        b = self.box()
        (b.d / 'uid').write_text('1000')
        a, tid, root, svc = self.team()
        r = self.deploy(a, tid, b, ok=False)
        code, why = r.stdout.strip().split('\t')
        self.assertEqual(code, 'root')
        self.assertIn('root@box 不是 root，sudo 又要密码', why)
        self.assertFalse((b.root / 'opt' / svc).exists())
        b.flag('sudo_ok')
        r = self.deploy(a, tid, b, '--port', str(free_port()))
        self.assertEqual(kv(r.stdout)['state'], 'new')
        self.assertIn('-n sh -s', b.log('sudo.log'))

    def test_dropped_after_start(self):
        """服务已经用这一次的设置起来了、打结果时 SSH 断了（SIGPIPE）：服务器上不换回（换回的话跑着的和 .env 对不上），这台设备照旧接着
        原来的；再部署是更新，接上新的"""
        b = self.box()
        a, tid, root, svc = self.team()
        d = b.root / 'opt' / svc
        port = free_port()
        self.deploy(a, tid, b, '--port', str(port))
        a.fake('ssh', SSH.replace('exec "$HUB_ENTER" "$cmd"', '"$HUB_ENTER" "$cmd" | true; exit 255'))
        p2 = free_port()
        r = self.deploy(a, tid, b, '--port', str(p2), ok=False)
        self.assertEqual(r.stdout.split('\t')[0], 'ssh')
        self.assertIn('或者中途断了', r.stdout)
        self.assertIn('Team 的同步服务器在运行', r.stderr, '服务起来了')
        self.assertEqual(b.envfile(d)['CCA_HUB_PORT'], str(p2), '服务器上是这一次的')
        self.assertFalse((d / '.env.prev').exists())
        self.assertTrue(b.running(svc))
        self.assertEqual(json.loads((a.cca / 'teams' / tid / 'team.json').read_text())['url'], 'https://127.0.0.1:%d' % port)
        a.fake('ssh', SSH)
        r = self.deploy(a, tid, b)
        self.assertEqual([kv(r.stdout)[k] for k in ('state', 'port')], ['update', str(p2)])
        self.assertEqual(json.loads((a.cca / 'teams' / tid / 'team.json').read_text())['url'], 'https://127.0.0.1:%d' % p2)

    def test_login_check_and_password(self):
        """App 里部署（bash 的 hub_deploy_ask）：--check 只看登录得上不（什么都不装、不起服务），ssh 说的分成几种原因（bash 照它重问、问密码、
        停下）；--askpass 经 SSH_ASKPASS 给密码（不用密钥、只问一次，有 --batch 也一样）；没有终端时不问要不要认这台主机（accept-new）"""
        b = self.box()
        a, tid, root, svc = self.team()
        pw = 'pw-' + secrets.token_hex(8)
        (a.home / 'server-pw').write_text(pw)
        a.fake('ssh', SSH_PW)
        a.fake('askpass', 'cat "%s"\n' % (a.home / 'given'))   # bash 的 askpass_run 那样：打出给它的密码
        ask = dict(HUB_PW=str(a.home / 'server-pw'), SSH_ASKPASS=str(a.home / 'bin' / 'askpass'), SSH_ASKPASS_REQUIRE='force')

        def check(*args, given=None):
            (a.home / 'given').write_text(given or '')
            (a.home / 'ssh.argv').unlink(missing_ok=True)
            r = self.deploy(a, tid, b, *args, ok=False, **ask)
            return r, (a.home / 'ssh.argv').read_text() if (a.home / 'ssh.argv').exists() else ''

        r, argv = check('--batch', '--check')
        self.assertEqual((r.returncode, r.stdout.split('\t')[0]), (1, 'ssh_auth'), '密钥登录不了、服务器认密码：bash 接着问密码')
        self.assertIn('要用密码登录', r.stdout)
        for o in ('BatchMode=yes', 'StrictHostKeyChecking=accept-new', 'LogLevel=ERROR', '-T root@box true'):
            self.assertIn(o, argv)
        r, argv = check('--batch', '--askpass', '--check', given='wrong-' + pw)
        self.assertEqual(r.stdout, 'ssh_denied\t密码不对\n')
        for o in ('PubkeyAuthentication=no', 'PreferredAuthentications=keyboard-interactive,password', 'NumberOfPasswordPrompts=1',
                  'StrictHostKeyChecking=accept-new'):
            self.assertIn(o, argv)
        self.assertNotIn('BatchMode', argv, '用密码时不能是 BatchMode（ssh 就不问 SSH_ASKPASS 了）')
        r, argv = check('--batch', '--askpass', '--check', given=pw)
        self.assertEqual((r.returncode, r.stdout, r.stderr), (0, '', ''), '密码对了：什么都不打')
        self.assertFalse((b.root / 'opt' / svc).exists(), '--check 不装')
        (a.home / 'askpass.n').unlink()
        r, argv = check('--batch', '--askpass', '--port', str(free_port()), given=pw)
        self.assertEqual((r.returncode, kv(r.stdout)['state']), (0, 'new'), r.stdout + r.stderr)
        self.assertEqual((a.home / 'askpass.n').read_text(), 'x\n', '部署只连一次（-G 不登录），只取一次密码')
        self.assertNotIn(pw, argv + b.files_text(), '密码不在 ssh 的命令行和服务器上')
        self.assertNotIn(pw, r.stdout + r.stderr)
        a.fake('ssh', SSH)   # 服务器认这台设备的密钥了；不认、也不让用密码：说怎么办
        b.flag('ssh_denied')
        r, _ = check('--batch', '--check')
        self.assertEqual(r.stdout.split('\t')[0], 'ssh_keyonly')
        self.assertIn('authorized_keys', r.stdout)
        b.flag('ssh_denied', False)
        a.fake('ssh', SSH_SAYS)
        for says, code, part in (('@@@@@@@@@@@\n@    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @\nHost key verification failed.', 'ssh_hostkey', 'ssh-keygen -R'),
                                 ('ssh: Could not resolve hostname box: nodename nor servname provided, or not known', 'ssh',
                                  'SSH 连不上 root@box：ssh: Could not resolve hostname box'),
                                 ('', 'ssh', 'SSH 连不上 root@box'),
                                 # ssh-agent 里的密钥太多：服务器只让试几把（MaxAuthTries），还没轮到密码就断了（LogLevel=ERROR 时最后一句就是这个）
                                 ('Received disconnect from 192.0.2.10 port 22:2: Too many authentication failures', 'ssh_auth', '要用密码登录')):
            r = self.deploy(a, tid, b, '--batch', '--check', ok=False, HUB_SSH_SAYS=says)
            self.assertEqual((r.returncode, r.stdout.split('\t')[0]), (1, code), says)
            self.assertIn(part, r.stdout)
        r = self.deploy(a, tid, b, '--batch', '--askpass', '--check', ok=False, HUB_SSH_SAYS='root@box: Permission denied (publickey).')
        self.assertEqual((r.returncode, r.stdout.split('\t')[0]), (1, 'ssh_keyonly'), '用密码时服务器不让用密码：不说密码不对')
        self.assertIn('IdentitiesOnly', r.stdout)

    def test_ssh_and_cert(self):
        """ssh 连不上、登录不了；服务器上的证书没了（重新生成）：照新的证书重新接上"""
        b = self.box()
        a, tid, root, svc = self.team()
        d = b.root / 'opt' / svc
        b.flag('ssh_denied')
        r = self.deploy(a, tid, b, '--batch', ok=False)
        code, why = r.stdout.strip().split('\t')
        self.assertEqual(code, 'ssh')
        self.assertIn('在终端里运行 cca team deploy', why)
        self.assertIn('BatchMode=yes', (a.home / 'ssh.argv').read_text())
        self.assertNotIn('accept-new', (a.home / 'ssh.argv').read_text(), '命令行里没有终端：没见过的主机照旧不连（只有 App 里先查登录时认下）')
        b.flag('ssh_denied', False)
        self.deploy(a, tid, b, '--port', str(free_port()))
        old = pin(d)
        b.stop()
        for f in ('key.pem', 'cert.pem'):
            (d / 'certs' / f).unlink()
        self.deploy(a, tid, b)
        self.assertNotEqual(pin(d), old)
        self.assertEqual(json.loads((a.cca / 'teams' / tid / 'team.json').read_text())['pins'], [pin(d)])
        self.assertEqual(a.ok('team-sync').split('\t')[2], 'ok')


if __name__ == '__main__':
    unittest.main(verbosity=1)
