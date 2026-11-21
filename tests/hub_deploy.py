#!/usr/bin/env python3
"""自己的同步服务器的一键部署（docs/dev/hub.md「部署」）。辅助程序生成的安装脚本（hub-installer）在 tests/hub_box.py 的假服务器上运行
（临时目录当作 /，假的 systemctl、docker 起真的 server/cca-presence.py）。有 /bin/dash（Debian、Ubuntu 的 sh）就用它运行，有的用例用
/bin/sh（macOS、Fedora 上是 bash，Alpine 上是 busybox）；tests/linux.sh 在各个发行版的容器里也跑这一份。
装好以后设备（tests/presence.py 的 Device：临时 HOME、假 curl）用辅助程序真的加入。cca hub deploy 的那一半（hub-deploy）经
假的 ssh 在同一台假服务器上运行脚本（ssh 的命令行、服务器上的文件里都不能有入伙口令和盐）。"""
import json
import os
from pathlib import Path
import re
import secrets
import shlex
import signal
import subprocess
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from helper_bin import HELPER   # noqa: E402
from hub_box import SH, SS, UFW, Box, free_port   # noqa: E402
from presence import Device, openssl_pin, wait_for   # noqa: E402
import spec_wire as sw   # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
TEMPLATE = ROOT / 'server' / 'hub-install.sh'
IMAGES = ['public.ecr.aws/docker/library/python:3.13-slim', 'mirror.ccs.tencentyun.com/library/python:3.13-slim']
LINK_RE = re.compile(r'cca-hub1\.[A-Za-z0-9_-]+\.[0-9a-f]{4}')
# 设备上的 ssh：-G 说这台假服务器在 127.0.0.1；别的把最后一个参数（在服务器上运行的命令）交给假服务器的 enter（stdin 是安装脚本）
SSH = r'''printf '%s\n' "$*" >> "$HOME/ssh.argv"
for a in "$@"; do [ "$a" = -G ] && { printf 'user root\nhostname 127.0.0.1\nport 22\n'; exit 0; }; done
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


def pin(box, d=None):
    return openssl_pin((d or box.dir) / 'certs' / 'cert.pem')


def kv(out):
    """hub-deploy 成了时的输出：sh 的 键=值，一行一项"""
    return dict(shlex.split(l)[0].split('=', 1) for l in out.splitlines() if l)


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

    def device(self, name, table=None):
        dev = Device(self.t / ('%s-%s' % (name, secrets.token_hex(4))), name, table or {'bob(bob@example.com)': 'US-B'})
        dev.config(lambda c: c.pop('policy'))   # 没打开维护者的共享：只有自己的同步服务器
        return dev

    def link_of(self, out):
        m = LINK_RE.findall(out)
        self.assertEqual(len(m), 1, out)
        return m[0]

    def join(self, dev, link):
        r = dev.run('hub-join', stdin=link)
        self.assertEqual((r.returncode, r.stderr), (0, ''), r.stdout)
        return r.stdout.splitlines()


class InstallerTest(Base):
    """在服务器上直接运行安装脚本（sudo sh cca-hub-install.sh）"""
    def test_script_itself(self):
        """生成的脚本：sh -n 过得去、带着版本和服务端程序；模板本身不能运行；参数、root 的检查（什么都不动）"""
        s = self.script.read_text()
        for sh in sorted({SH, '/bin/sh'}):
            self.assertEqual(subprocess.run([sh, '-n', str(self.script)]).returncode, 0, sh)
        self.assertIn("CCA_HUB_VERSION='9.9.9'", s)
        self.assertIn((ROOT / 'server' / 'cca-presence.py').read_text(), s)
        self.assertNotIn('CCA_HUB_MANAGED=1', s, '直接在服务器上运行的那一份不带 hub-deploy 的那几项')
        b = self.box()
        r = b.install(TEMPLATE)
        self.assertEqual(r.returncode, 1)
        self.assertIn('模板', r.stderr)
        r = b.install(self.script, '--port', 'abc')
        self.assertEqual((r.returncode, r.stderr.strip()), (1, '✗ --port 要是 1024～65535 的端口'))
        r = b.install(self.script, '--dir', 'relative')
        self.assertIn('绝对路径', r.stderr)
        for d in ('/', '//', '/.', '/opt/../etc', '/a//b', '/a b'):   # 会落到 / 或者别处：卸载时删的是目录里的 data 这些
            r = b.install(self.script, '--dir', d)
            self.assertEqual(r.returncode, 1, d)
            self.assertIn('要是 / 下面的一个目录', r.stderr, d)
        for d in ('/home/me/hub', '/home', '/etc/cca-hub', '/usr/local/hub/', '/tmp/hub', '/run/hub'):   # systemd 的服务看不到、写不了
            r = b.install(self.script, '--dir', d)
            self.assertEqual(r.returncode, 1, d)
            self.assertIn('systemd 方式的服务看不到', r.stderr, d)
        r = b.install(self.script, '--bogus')
        self.assertEqual(r.returncode, 2)
        self.assertIn('用法', r.stderr)
        (b.d / 'uid').write_text('1000')
        r = b.install(self.script)
        self.assertEqual(r.returncode, 1)
        self.assertIn('要用 root 运行', r.stderr)
        self.assertEqual(sorted(p.name for p in b.root.iterdir()), ['etc', 'run'], '什么都没动')

    def test_systemd(self):
        """systemd + python3：装好、打出加入链接（服务器上只有口令的哈希）、设备加入；再运行是更新；--reset 换链接；--uninstall"""
        b = self.box()
        port = free_port()
        r = b.install(self.script, '--host', '127.0.0.1', '--port', str(port))
        self.assertEqual((r.returncode, r.stderr), (0, ''), r.stdout)
        link = self.link_of(r.stdout)
        doc = sw.link_decode(link)
        self.assertEqual(doc['u'], 'https://127.0.0.1:%d' % port)
        self.assertEqual(doc['p'], [pin(b)])
        self.assertRegex(doc['a'], '^[0-9a-f]{32}$')
        self.assertRegex(doc['s'], '^[0-9a-f]{32}$')
        self.assertNotEqual(doc['a'], doc['s'])
        e = b.envfile()
        self.assertEqual((e['CCA_HUB_MODE'], e['CCA_HUB_PORT'], e['CCA_HUB_ADMIT_HASH'], e['CCA_HUB_VERSION']),
                         ('systemd', str(port), sw.admit_hash(doc['a']), '9.9.9'))
        self.assertNotIn('CCA_HUB_IMAGE', e)
        files = b.files_text()
        self.assertNotIn(doc['a'], files, '服务器上不存入伙口令')
        self.assertNotIn(doc['s'], files, '盐不落盘')
        modes = {p: oct((b.dir / p).stat().st_mode & 0o777)
                 for p in ('.', '.env', '.cca-hub', 'app', 'certs', 'certs/key.pem', 'certs/cert.pem', 'data')}
        self.assertEqual(modes, {'.': '0o755', '.env': '0o600', '.cca-hub': '0o644', 'app': '0o755', 'certs': '0o700',
                                 'certs/key.pem': '0o600', 'certs/cert.pem': '0o644', 'data': '0o700'})
        self.assertFalse((b.dir / '.env.prev').exists(), '装好了：不留原来的 .env')
        unit = (b.root / 'etc' / 'systemd' / 'system' / 'cca-hub.service').read_text()
        for want in ('User=cca-hub', 'EnvironmentFile=/opt/cca-hub/.env', 'NoNewPrivileges=yes', 'ProtectSystem=full',
                     '--mode own --listen 0.0.0.0:${CCA_HUB_PORT} --cert /opt/cca-hub/certs/cert.pem'):
            self.assertIn(want, unit)
        self.assertRegex(unit, r'ExecStart=/\S+python3\S* -I /opt/cca-hub/app/cca-presence.py ')
        self.assertIn('-r -M -d /opt/cca-hub', b.log('useradd.log'))
        self.assertIn('-R cca-hub %s/certs %s/data' % (b.dir, b.dir), b.log('chown.log'))
        self.assertEqual(b.log('systemctl.log').split('\n')[:3], ['daemon-reload', 'enable cca-hub', 'restart cca-hub'])
        self.assertIn('云服务商的安全组', r.stdout)
        self.assertIn('输出没接在终端上', r.stdout, 'stdout 不是终端：说一声链接可能进了日志')
        self.assertEqual((b.dir / 'app' / 'cca-presence.py').read_text(), (ROOT / 'server' / 'cca-presence.py').read_text())

        a = self.device('alice-mbp')
        self.assertEqual(self.join(a, link)[1:3], ['127.0.0.1:%d' % port, '1'])
        # 再运行：只更新，不打链接；服务重启过，已经加入的照样用
        r = b.install(self.script)
        self.assertEqual((r.returncode, r.stderr), (0, ''), r.stdout)
        self.assertIn('更新了服务端程序', r.stdout)
        self.assertEqual(LINK_RE.findall(r.stdout), [])
        self.assertEqual(b.envfile()['CCA_HUB_ADMIT_HASH'], sw.admit_hash(doc['a']))
        self.assertEqual(b.envfile()['CCA_HUB_PORT'], str(port), '没给 --port：照上次的')
        self.assertEqual(a.run('hub-devices').returncode, 0)
        r = b.install(self.script, '--port', str(free_port()))
        self.assertEqual(r.returncode, 1)
        self.assertIn('换端口得 --reset', r.stderr)
        # --reset：新的链接，以前加入的设备服务器不认了
        r = b.install(self.script, '--reset', '--host', '127.0.0.1')
        self.assertEqual((r.returncode, r.stderr), (0, ''), r.stdout)
        link2 = self.link_of(r.stdout)
        self.assertNotEqual(sw.link_decode(link2)['a'], doc['a'])
        self.assertEqual(sw.link_decode(link2)['p'], doc['p'], '证书照旧')
        self.assertIn('先 cca hub leave', r.stdout)
        wait_for('服务器重启后不认 alice', lambda: a.run('hub-devices').returncode == 1)
        self.assertEqual(a.run('hub-join', stdin=link2).stdout.split('\t')[0], 'other', '先退出才能加入新的')
        # 卸载：停掉、删掉服务器上的文件和用户
        r = b.install(self.script, '--uninstall')
        self.assertEqual((r.returncode, r.stderr), (0, ''), r.stdout)
        self.assertFalse(b.dir.exists())
        self.assertFalse((b.root / 'etc' / 'systemd' / 'system' / 'cca-hub.service').exists())
        self.assertFalse(b.running())
        self.assertEqual(list((b.d / 'users').iterdir()), [])
        self.assertIn('disable --now cca-hub', b.log('systemctl.log'))

    def test_docker(self):
        """Docker compose：用第一个拉得到的镜像，容器里的用户 10001；拉不到时自动改用 systemd，指定了 --docker 就停下"""
        b = self.box(docker=True)
        port = free_port()
        r = b.install(self.script, '--host', '127.0.0.1', '--port', str(port), shell='/bin/sh')
        self.assertEqual((r.returncode, r.stderr), (0, ''), r.stdout)
        link = self.link_of(r.stdout)
        e = b.envfile()
        self.assertEqual((e['CCA_HUB_MODE'], e['CCA_HUB_IMAGE']), ('docker', IMAGES[0]))
        self.assertIn('Docker compose，镜像 ' + IMAGES[0], r.stdout)
        y = (b.dir / 'compose.yaml').read_text()
        for want in ('container_name: cca-hub', 'user: "10001:10001"', 'read_only: true', 'cap_drop: [ALL]', '- ./data:/data:z'):
            self.assertIn(want, y)
        self.assertIn('-R 10001:10001', b.log('chown.log'))
        self.assertIn('compose -f compose.yaml up -d --force-recreate --remove-orphans', b.log('docker.log'))
        self.assertFalse((b.root / 'etc' / 'systemd' / 'system' / 'cca-hub.service').exists())
        self.assertEqual(b.log('useradd.log'), '')
        a = self.device('alice-mbp')
        self.assertEqual(self.join(a, link)[1], '127.0.0.1:%d' % port)
        # 改用 systemd（--systemd）：停掉容器，数据和加入链接照旧
        r = b.install(self.script, '--systemd')
        self.assertEqual((r.returncode, r.stderr), (0, ''), r.stdout)
        self.assertIn('compose -f compose.yaml down', b.log('docker.log'))
        self.assertFalse((b.dir / 'compose.yaml').exists())
        self.assertEqual(b.envfile()['CCA_HUB_MODE'], 'systemd')
        self.assertEqual(a.run('hub-devices').returncode, 0, '换了运行方式，已经加入的照样用')
        r = b.install(self.script)
        self.assertIn('systemd + python', r.stdout, '没指定：照上次的')

        # 拉不到镜像
        c = self.box(docker=True)
        c.flag('no_pull')
        r = c.install(self.script, '--host', '127.0.0.1', '--port', str(free_port()))
        self.assertEqual((r.returncode, r.stderr), (0, ''), r.stdout)
        self.assertIn('改用 systemd', r.stdout)
        self.assertEqual(c.envfile()['CCA_HUB_MODE'], 'systemd')
        d = self.box(docker=True)
        d.flag('no_pull')
        r = d.install(self.script, '--docker', '--host', '127.0.0.1', '--port', str(free_port()))
        self.assertEqual(r.returncode, 1)
        self.assertIn('拉不到 python:3.13-slim 镜像', r.stderr)
        self.assertFalse(d.dir.exists(), '没拉到镜像：什么都还没动')

    def test_compose(self):
        """只用 compose 插件：只有老的 docker-compose 1.x 时改用 systemd（--docker 时停下）；compose up 起不来时打出它说的、
        改用 systemd（--docker 时停下，第一次装的 .env 不留）；Docker 的卸载"""
        b = self.box(docker=True)
        b.flag('no_compose')
        r = b.install(self.script, '--host', '127.0.0.1', '--port', str(free_port()))
        self.assertEqual((r.returncode, r.stderr), (0, ''), r.stdout)
        self.assertEqual(b.envfile()['CCA_HUB_MODE'], 'systemd')
        r = b.install(self.script, '--docker')
        self.assertEqual(r.returncode, 1)
        self.assertIn('带 compose 插件', r.stderr)
        self.assertEqual(b.log('docker-compose.log'), '', '不用老的 docker-compose')

        c = self.box(docker=True)
        c.flag('compose_fail')
        r = c.install(self.script, '--host', '127.0.0.1', '--port', str(free_port()))
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn('    Error response from daemon', r.stderr, 'compose 说的打出来（缩进）')
        self.assertIn('Docker compose 起不来（上面是它说的）：改用 systemd', r.stdout)
        self.assertEqual(c.envfile()['CCA_HUB_MODE'], 'systemd')
        self.assertNotIn('CCA_HUB_IMAGE', c.envfile())
        self.assertFalse((c.dir / 'compose.yaml').exists())
        self.assertIn('compose -f compose.yaml down', c.log('docker.log'))
        self.assertTrue(c.running())
        self.link_of(r.stdout)

        d = self.box(docker=True)
        d.flag('compose_fail')
        r = d.install(self.script, '--docker', '--host', '127.0.0.1', '--port', str(free_port()))
        self.assertEqual(r.returncode, 1)
        self.assertIn('Error response from daemon', r.stderr)
        self.assertIn('docker compose up 没成', r.stderr)
        self.assertFalse((d.dir / '.env').exists(), '第一次装没成：.env 不留（下一次还是第一次装）')
        d.flag('compose_fail', False)
        r = d.install(self.script, '--host', '127.0.0.1', '--port', str(free_port()))
        self.assertEqual((r.returncode, r.stderr), (0, ''), r.stdout)
        self.assertEqual(d.envfile()['CCA_HUB_MODE'], 'docker')
        self.link_of(r.stdout)
        r = d.install(self.script, '--uninstall')
        self.assertEqual((r.returncode, r.stderr), (0, ''), r.stdout)
        self.assertFalse(d.dir.exists())
        self.assertFalse(d.running())

    def test_dir(self):
        """--dir：别人的目录（里面有东西）不碰，卸载也不碰；装在自己的目录（末尾的 / 去掉）；一台服务器只装一个"""
        b = self.box()
        precious = b.root / 'srv' / 'data' / 'precious'
        precious.parent.mkdir(parents=True)
        precious.write_text('别人的数据')
        r = b.install(self.script, '--dir', '/srv', '--host', '127.0.0.1')
        self.assertEqual(r.returncode, 1)
        self.assertIn('/srv 里已经有别的东西', r.stderr)
        r = b.install(self.script, '--uninstall', '--dir', '/srv')
        self.assertEqual(r.returncode, 1)
        self.assertIn('/srv 里没有装自己的同步服务器', r.stderr)
        self.assertEqual((precious.read_text(), b.log('chown.log')), ('别人的数据', ''))
        port = free_port()
        r = b.install(self.script, '--dir', '/srv/cca-hub/', '--host', '127.0.0.1', '--port', str(port))
        self.assertEqual((r.returncode, r.stderr), (0, ''), r.stdout)
        hub = b.root / 'srv' / 'cca-hub'
        self.assertTrue((hub / '.cca-hub').is_file())
        self.assertIn('EnvironmentFile=/srv/cca-hub/.env\n', (b.root / 'etc' / 'systemd' / 'system' / 'cca-hub.service').read_text())
        a = self.device('alice-mbp')
        self.join(a, self.link_of(r.stdout))
        r = b.install(self.script, '--host', '127.0.0.1')   # 默认的 /opt/cca-hub：这台已经在 /srv/cca-hub 装了一个
        self.assertEqual(r.returncode, 1)
        self.assertIn('已经在 /srv/cca-hub 装了', r.stderr)
        self.assertFalse(b.dir.exists())
        r = b.install(self.script, '--dir', '/srv/cca-hub')
        self.assertIn('更新了服务端程序', r.stdout)
        self.assertEqual(a.run('hub-devices').returncode, 0)
        r = b.install(self.script, '--uninstall', '--dir', '/srv/cca-hub')
        self.assertEqual((r.returncode, r.stderr), (0, ''), r.stdout)
        self.assertFalse(hub.exists())
        self.assertEqual(precious.read_text(), '别人的数据', '旁边的不动')

    def test_failed_reset_rolls_back(self):
        """--reset 起不来（.env 已经换了）、被打断：.env 换回原来的（口令、端口），再运行（不带 --reset）照旧是更新，加入了的设备照样用"""
        b = self.box()
        port = free_port()
        r = b.install(self.script, '--host', '127.0.0.1', '--port', str(port))
        a = self.device('alice-mbp')
        self.join(a, self.link_of(r.stdout))
        before = (b.dir / '.env').read_text()
        b.flag('start_fail')
        r = b.install(self.script, '--reset', '--host', '127.0.0.1', '--port', str(free_port()))
        self.assertEqual(r.returncode, 1)
        self.assertIn('systemd 起不来', r.stderr)
        self.assertEqual((b.dir / '.env').read_text(), before, '.env 换回了原来的')
        self.assertFalse((b.dir / '.env.prev').exists())
        b.flag('start_fail', False)
        # 被打断（Ctrl-C、SSH 断了）：在起服务的时候
        b.flag('start_hang')
        restarts = b.log('systemctl.log').count('restart cca-hub')   # 上面起不来的那一次最后也是 restart：要等这一次的
        p = subprocess.Popen([SH, str(self.script), '--reset', '--host', '127.0.0.1'], env=b.env, stdin=subprocess.DEVNULL,
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
        try:
            wait_for('起服务', lambda: b.log('systemctl.log').count('restart cca-hub') > restarts and (b.dir / '.env.prev').exists())
            self.assertNotEqual((b.dir / '.env').read_text(), before)
            os.killpg(p.pid, signal.SIGTERM)
            p.communicate(timeout=30)
        finally:
            if p.poll() is None:
                os.killpg(p.pid, signal.SIGKILL)
                p.communicate()
        self.assertEqual(p.returncode, 1)
        self.assertEqual((b.dir / '.env').read_text(), before, '被打断：.env 换回了原来的')
        b.flag('start_hang', False)
        r = b.install(self.script)
        self.assertEqual((r.returncode, r.stderr), (0, ''), r.stdout)
        self.assertIn('更新了服务端程序', r.stdout)
        self.assertEqual(b.envfile()['CCA_HUB_PORT'], str(port))
        wait_for('重启后照样认 alice', lambda: a.run('hub-devices').returncode == 0)

    def test_rerun_before_anyone_joined(self):
        """还没有设备加入过：再运行换一个新的加入链接（第一个丢了、端口没放行时），旧的作废"""
        b = self.box()
        port = free_port()
        link1 = self.link_of(b.install(self.script, '--host', '127.0.0.1', '--port', str(port)).stdout)
        r = b.install(self.script, '--host', '127.0.0.1')
        self.assertEqual(r.returncode, 0, r.stderr)
        link2 = self.link_of(r.stdout)
        self.assertNotEqual(link1, link2)
        a = self.device('alice-mbp')
        wait_for('重启后的服务器不认旧链接', lambda: a.run('hub-join', stdin=link1).stdout.startswith('denied\t'))
        self.assertEqual(self.join(a, link2)[2], '1')

    def test_needs(self):
        """缺东西时停下、说怎么办，什么都不动：没有 Docker 也没有 systemd + python3、Python 太旧、端口被占、认不出公网地址"""
        b = self.box(systemd=False)
        r = b.install(self.script)
        self.assertEqual(r.returncode, 1)
        self.assertIn('两样都没有', r.stderr)
        c = self.box()
        c.fake('python3', '#!/bin/sh\nexit 1\n')   # 3.6 以前的、没有 ssl 的
        r = c.install(self.script, '--systemd')
        self.assertIn('要 systemd 和 python3（3.6 起）', r.stderr)
        d = self.box()
        port = free_port()
        (d.d / 'listening').write_text('%d\n' % port)
        d.fake('ss', '#!/bin/sh\n' + SS)
        r = d.install(self.script, '--host', '127.0.0.1', '--port', str(port))
        self.assertIn('端口 %d 被别的程序占着' % port, r.stderr)
        # 公网地址：机器上看得到的才认；内网的（云服务器的公网 IP 常常是 NAT 进来的）又没有终端可问时要 --host
        (d.d / 'ip').write_text('10.9.0.5')
        r = d.install(self.script, '--port', str(free_port()))
        self.assertIn('加上 --host', r.stderr)
        for x in (b, c, d):
            self.assertFalse(x.dir.exists())
        (d.d / 'ip').write_text('203.0.113.7')
        r = d.install(self.script, '--port', str(free_port()))
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertTrue(sw.link_decode(self.link_of(r.stdout))['u'].startswith('https://203.0.113.7:'))


class DeployTest(Base):
    """cca hub deploy（hub-deploy）：经假的 ssh 把同一个脚本交给假服务器；入伙口令、盐在设备上生成，设备跟着加入"""
    def deploy(self, dev, box, *args, ok=True):
        r = dev.run('hub-deploy', 'root@box', *args, HUB_ENTER=str(box.enter), CCA_VERSION='9.9.9')
        if ok:
            self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        return r

    def device(self, name, table=None):
        dev = super().device(name, table)
        dev.fake('ssh', SSH)
        return dev

    def secrets_of(self, dev):
        return [json.loads((dev.cca / 'hub' / 'hub.json').read_text())[k] for k in ('admit', 'salt')]

    def test_deploy(self):
        b = self.box(docker=True)
        port = free_port()
        a = self.device('alice-mbp')
        r = self.deploy(a, b, '--port', str(port), '--name', 'alice-desk')
        self.assertEqual(kv(r.stdout), {'mode': 'docker', 'state': 'new', 'port': str(port), 'dir': '/opt/cca-hub', 'lan': '1',
                                        'name': 'alice-desk', 'host': '127.0.0.1:%d' % port, 'members': '1', 'off': '', 'newpin': ''},
                         '127.0.0.1 只在本机连得上：lan')
        self.assertIn('自己的同步服务器在运行', r.stderr, '服务器那边的进度经 ssh 的 stderr 直接打出来')
        admit, salt = self.secrets_of(a)
        self.assertEqual(b.envfile()['CCA_HUB_ADMIT_HASH'], sw.admit_hash(admit))
        files, argv = b.files_text(), (a.home / 'ssh.argv').read_text()
        for s in (admit, salt):
            self.assertNotIn(s, files, '服务器上没有口令和盐')
            self.assertNotIn(s, argv, 'ssh 的命令行里没有')
        self.assertIn('-o ConnectTimeout=15 -G root@box', argv)
        self.assertIn('-o ConnectTimeout=15 -T -o ServerAliveInterval=30 root@box sh -c', argv, '不要伪终端：脚本从 stdin 过去')
        inv = a.ok('hub-invite').strip()
        self.assertEqual(sw.link_decode(inv), {'u': 'https://127.0.0.1:%d' % port, 'p': [pin(b)], 'a': admit, 's': salt})
        # 再部署（加入了的）：只更新，照旧加入着
        r = self.deploy(a, b)
        self.assertEqual([kv(r.stdout)[k] for k in ('mode', 'state', 'port', 'name')], ['docker', 'update', str(port), 'alice-desk'])
        self.assertEqual(self.secrets_of(a), [admit, salt])
        r = self.deploy(a, b, '--port', str(port + 1), ok=False)
        self.assertEqual(r.stdout.split('\t')[0], 'moved')
        # 另一台没加入的设备：服务器上已经有设备加入过，不动它；--reset 从头来
        c = self.device('carol-air')
        r = self.deploy(c, b, ok=False)
        self.assertEqual(r.returncode, 1)
        self.assertEqual(r.stdout.split('\t')[0], 'taken')
        self.assertIn('cca hub invite', r.stdout)
        self.assertEqual(b.envfile()['CCA_HUB_ADMIT_HASH'], sw.admit_hash(admit))
        r = self.deploy(c, b, '--reset')
        self.assertEqual([kv(r.stdout)[k] for k in ('mode', 'state')], ['docker', 'reset'])
        self.assertNotEqual(self.secrets_of(c)[0], admit)
        c_name = kv(r.stdout)['name']
        # 原来那台：它加入的那一份服务器上已经没了；也不让它 --reset（服务器上现在的不是它加入的那一份）：先退出
        for args in ((), ('--reset',)):
            r = self.deploy(a, b, *args, ok=False)
            self.assertEqual(r.stdout.split('\t')[0], 'not_this', args)
            self.assertIn('先 cca hub leave', r.stdout)
        self.assertEqual(b.envfile()['CCA_HUB_ADMIT_HASH'], sw.admit_hash(self.secrets_of(c)[0]), 'c 的那一份没动')
        # c 再 --reset（加入着的就是这一台）：本机的旧加入信息换掉，用新的加入；端口可以换，名字照旧
        old_c = self.secrets_of(c)
        r = self.deploy(c, b, '--reset', '--port', str(port + 2))
        o = kv(r.stdout)
        self.assertEqual((o['state'], o['port'], o['members'], o['name']), ('reset', str(port + 2), '1', c_name))
        self.assertNotEqual(self.secrets_of(c), old_c)
        self.assertEqual(json.loads((c.cca / 'hub' / 'hub.json').read_text())['url'], 'https://127.0.0.1:%d' % (port + 2))

    def test_reset_keeps_the_name(self):
        """加入了的设备 --reset：名字照旧（--name 给过的）；起不来时本机照旧加入着原来的那一份、服务器的 .env 换回原来的，
        再部署（不带 --reset）是更新"""
        b = self.box()
        a = self.device('alice-mbp')
        self.deploy(a, b, '--port', str(free_port()), '--name', 'alice-desk')
        mine, before = (a.cca / 'hub' / 'hub.json').read_text(), (b.dir / '.env').read_text()
        b.flag('start_fail')
        r = self.deploy(a, b, '--reset', ok=False)
        self.assertEqual(r.stdout.split('\t')[0], 'start')
        self.assertEqual(((a.cca / 'hub' / 'hub.json').read_text(), (b.dir / '.env').read_text()), (mine, before))
        b.flag('start_fail', False)
        r = self.deploy(a, b)
        self.assertEqual([kv(r.stdout)[k] for k in ('state', 'name')], ['update', 'alice-desk'])
        r = self.deploy(a, b, '--reset')
        self.assertEqual([kv(r.stdout)[k] for k in ('state', 'name', 'members')], ['reset', 'alice-desk', '1'])

    def test_dropped_after_start(self):
        """--reset 时服务已经用新的口令起来了、打结果时 SSH 断了（SIGPIPE）：服务器上不换回（换回的话跑着的和 .env 对不上，旧链接下次
        重启又能用），本机照旧记着原来的；再部署是 not_this，先 hub leave 再 --reset 就好了"""
        b = self.box()
        a = self.device('alice-mbp')
        self.deploy(a, b, '--port', str(free_port()))
        mine, old = (a.cca / 'hub' / 'hub.json').read_text(), b.envfile()['CCA_HUB_ADMIT_HASH']
        a.fake('ssh', SSH.replace('exec "$HUB_ENTER" "$cmd"', '"$HUB_ENTER" "$cmd" | true; exit 255'))
        r = self.deploy(a, b, '--reset', ok=False)
        self.assertEqual(r.stdout.split('\t')[0], 'ssh')
        self.assertIn('或者中途断了', r.stdout)
        self.assertIn('自己的同步服务器在运行', r.stderr, '服务起来了')
        new = b.envfile()['CCA_HUB_ADMIT_HASH']
        self.assertNotEqual(new, old, '服务器上是这一次的口令')
        self.assertFalse((b.dir / '.env.prev').exists())
        self.assertTrue(b.running())
        self.assertEqual((a.cca / 'hub' / 'hub.json').read_text(), mine)
        a.fake('ssh', SSH)
        r = self.deploy(a, b, '--reset', ok=False)
        self.assertEqual(r.stdout.split('\t')[0], 'not_this')
        self.assertEqual(b.envfile()['CCA_HUB_ADMIT_HASH'], new, '没动')
        self.assertEqual(a.run('hub-leave', 'apply').returncode, 0)
        r = self.deploy(a, b, '--reset')
        self.assertEqual([kv(r.stdout)[k] for k in ('state', 'members')], ['reset', '1'])

    def test_other_server(self):
        """加入了的设备不碰别的服务器：没装的、装着别人的，带不带 --reset 都是 not_this，那台什么都不动、本机照旧"""
        b, c, e = self.box(), self.box(), self.box()
        a = self.device('alice-mbp')
        self.deploy(a, b, '--port', str(free_port()))
        mine = (a.cca / 'hub' / 'hub.json').read_text()
        for args in ((), ('--reset',)):
            r = self.deploy(a, c, *args, ok=False)
            code, why = r.stdout.strip().split('\t')
            self.assertEqual(code, 'not_this', args)
            self.assertIn('这台设备加入的是别的服务器：要部署到这一台，先 cca hub leave', why)
            self.assertFalse(c.dir.exists())
        f = self.device('frank-pc')
        self.deploy(f, e, '--port', str(free_port()))
        theirs = (e.dir / '.env').read_text()
        r = self.deploy(a, e, '--reset', ok=False)
        self.assertEqual(r.stdout.split('\t')[0], 'not_this')
        self.assertIn('或者 cca hub deploy root@box --reset 从头来', r.stdout)
        self.assertEqual((e.dir / '.env').read_text(), theirs, '别人的没动')
        self.assertEqual((a.cca / 'hub' / 'hub.json').read_text(), mine, '本机加入的那一台照旧')

    def test_args(self):
        """参数不对：辅助程序先查（usage；名字不对是 name），不连服务器"""
        b = self.box()
        a = self.device('alice-mbp')
        for args in (('--port', 'abc'), ('--port', '80'), ('--port', '+2000'), ('--dir', '//'), ('--dir', '/opt/../etc'),
                     ('--dir', '/home/me/hub'), ('--host', 'a b'), ('--ssh-port', '0'), ('--mode', 'podman'), ('--bogus',), ('--port',)):
            r = self.deploy(a, b, *args, ok=False)
            self.assertEqual((r.returncode, r.stdout.split('\t')[0]), (1, 'usage'), args)
        r = self.deploy(a, b, '--name', ' ', ok=False)
        self.assertEqual(r.stdout.split('\t')[0], 'name')
        self.assertFalse((a.home / 'ssh.argv').exists(), '不连服务器')

    def test_unreachable_then_again(self):
        """装好了却连不上（安全组没放行）：说要放行哪个端口；放行以后再部署，还没有设备加入过，换一个新的链接、加入"""
        b = self.box()
        port = free_port()
        a = self.device('alice-mbp')
        (a.home / 'reply').mkdir()
        (a.home / 'reply' / 'join').write_text('000\n')   # 假 curl：连不上
        r = self.deploy(a, b, '--port', str(port), ok=False)
        code, why = r.stdout.strip().split('\t')
        self.assertEqual(code, 'unreachable')
        self.assertIn('服务器上装好了（127.0.0.1:%d），但这台设备没加入成' % port, why)
        self.assertIn('放行 TCP %d，放行以后再运行一次 cca hub deploy root@box' % port, why)
        self.assertFalse((a.cca / 'hub').exists(), '没加入成：本机什么都不留')
        first = b.envfile()['CCA_HUB_ADMIT_HASH']
        (a.home / 'reply' / 'join').unlink()
        b.fake('ufw', '#!/bin/sh\n' + UFW)
        r = self.deploy(a, b)
        self.assertEqual([kv(r.stdout)[k] for k in ('mode', 'state', 'port')], ['systemd', 'replace', str(port)])
        self.assertNotEqual(b.envfile()['CCA_HUB_ADMIT_HASH'], first)
        self.assertIn('这台服务器开着 ufw：运行 ufw allow %d/tcp 放行' % port, r.stderr, '服务器自己的防火墙：经 SSH 装时也说')

    def test_not_root(self):
        """用普通用户登录：免密码的 sudo 就经它装；sudo 要密码时什么都不运行，说要用 root 登录"""
        b = self.box()
        (b.d / 'uid').write_text('1000')
        a = self.device('alice-mbp')
        r = self.deploy(a, b, ok=False)
        code, why = r.stdout.strip().split('\t')
        self.assertEqual(code, 'root')
        self.assertIn('root@box 不是 root，sudo 又要密码', why)
        self.assertFalse(b.dir.exists())
        b.flag('sudo_ok')
        r = self.deploy(a, b, '--port', str(free_port()))
        self.assertEqual(kv(r.stdout)['state'], 'new')
        self.assertIn('-n sh -s', b.log('sudo.log'))

    def test_login_check_and_password(self):
        """App 里部署（bash 的 hub_deploy_ask）：--check 只看登录得上不（什么都不装、不起服务），ssh 说的分成几种原因（bash 照它重问、问密码、
        停下）；--askpass 经 SSH_ASKPASS 给密码（不用密钥、只问一次，有 --batch 也一样）；没有终端时不问要不要认这台主机（accept-new）"""
        b = self.box()
        a = self.device('alice-mbp')
        pw = 'pw-' + secrets.token_hex(8)
        (a.home / 'server-pw').write_text(pw)
        a.fake('ssh', SSH_PW)
        a.fake('askpass', 'cat "%s"\n' % (a.home / 'given'))   # bash 的 askpass_run 那样：打出给它的密码
        ask = dict(HUB_ENTER=str(b.enter), HUB_PW=str(a.home / 'server-pw'), SSH_ASKPASS=str(a.home / 'bin' / 'askpass'),
                   SSH_ASKPASS_REQUIRE='force', CCA_VERSION='9.9.9')

        def check(*args, given=None):
            (a.home / 'given').write_text(given or '')
            (a.home / 'ssh.argv').unlink(missing_ok=True)
            r = a.run('hub-deploy', 'root@box', *args, **ask)
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
        self.assertFalse(b.dir.exists(), '--check 不装')
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
            r = a.run('hub-deploy', 'root@box', '--batch', '--check', HUB_SSH_SAYS=says, CCA_VERSION='9.9.9')
            self.assertEqual((r.returncode, r.stdout.split('\t')[0]), (1, code), says)
            self.assertIn(part, r.stdout)
        r = a.run('hub-deploy', 'root@box', '--batch', '--askpass', '--check', HUB_SSH_SAYS='root@box: Permission denied (publickey).',
                  CCA_VERSION='9.9.9')
        self.assertEqual((r.returncode, r.stdout.split('\t')[0]), (1, 'ssh_keyonly'), '用密码时服务器不让用密码：不说密码不对')
        self.assertIn('IdentitiesOnly', r.stdout)

    def test_ssh_and_cert(self):
        """ssh 连不上、登录不了；服务器上的证书没了（重新生成）：加入了的设备换上新的指纹，说别的设备要新链接"""
        b = self.box()
        a = self.device('alice-mbp')
        b.flag('ssh_denied')
        r = self.deploy(a, b, '--batch', ok=False)
        code, why = r.stdout.strip().split('\t')
        self.assertEqual(code, 'ssh')
        self.assertIn('在终端里运行 cca hub deploy', why)
        self.assertIn('BatchMode=yes', (a.home / 'ssh.argv').read_text())
        self.assertNotIn('accept-new', (a.home / 'ssh.argv').read_text(), '命令行里没有终端：没见过的主机照旧不连（只有 App 里先查登录时认下）')
        b.flag('ssh_denied', False)
        port = free_port()
        self.deploy(a, b, '--port', str(port))
        old = pin(b)
        b.stop()
        for f in ('key.pem', 'cert.pem'):
            (b.dir / 'certs' / f).unlink()
        r = self.deploy(a, b)
        self.assertNotEqual(pin(b), old)
        self.assertEqual(kv(r.stdout)['newpin'], '1', '证书换了')
        self.assertEqual(json.loads((a.cca / 'hub' / 'hub.json').read_text())['pins'], [pin(b)])
        self.assertEqual(a.run('hub-devices').returncode, 0)


if __name__ == '__main__':
    unittest.main(verbosity=1)
