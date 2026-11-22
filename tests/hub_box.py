#!/usr/bin/env python3
"""一台假的 Linux 服务器，给自己的同步服务器的一键部署用（tests/hub_deploy.py；regress 11b3c 的 hubbox：python3 hub_box.py 目录）。
目录下的 root/ 当作 /（安装脚本的 CCA_HUB_ROOT），bin/ 里是假的系统命令：id、useradd、userdel、chown、ip、journalctl、sudo，
Python 写的 systemctl、docker（照 unit 的 ExecStart、compose.yaml 的 command 和挂载起真的 server/cca-presence.py，监听 127.0.0.1），
sh 是 /bin/dash（没有时 /bin/sh）。没有的 docker、systemctl 也放一个会失败的：不然会找到这台机器上真的。
enter 是假 ssh 的服务器那一头：在这台假服务器上运行一条命令（stdin 照样给它；目录下有 ssh_denied 时登录不了，它的内容是 once 时只拒这一次）。
目录下这些文件在时：uid（id -u 说的；root 是 0）、sudo_ok（免密码的 sudo）、ip（默认路由出去的源地址）、listening（ss 说这些端口有人监听）、
no_pull（docker pull 拉不到）、docker_down（docker info 不通）、no_compose（没有 compose 插件，只有老的 docker-compose 1.x）、
compose_fail（docker compose up 起不来）、start_fail（systemctl restart 起不来）、start_hang（systemctl restart 一直不回来）。"""
import os
from pathlib import Path
import socket
import subprocess
import sys

SH = '/bin/dash' if os.path.exists('/bin/dash') else '/bin/sh'   # 假服务器上的 sh

ID = r'''case "${1:-}" in
	-u) if [ -n "${HUB_SUDO:-}" ]; then echo 0; else cat "$HUB_BOX/uid" 2>/dev/null || echo 0; fi ;;
	'') echo 'uid=0(root) gid=0(root) groups=0(root)' ;;
	*) [ -f "$HUB_BOX/users/$1" ] ;;
esac
'''
USERADD = r'''printf '%s\n' "$*" >> "$HUB_BOX/useradd.log"
for a in "$@"; do u=$a; done
: > "$HUB_BOX/users/$u"
'''
USERDEL = 'rm -f "$HUB_BOX/users/$1"\n'
# sudo：sudo_ok 在时像免密码的 sudo（之后 id -u 是 0），不在时要密码（-n：不问，失败）。也免得测试里调到真的 sudo
SUDO = r'''printf '%s\n' "$*" >> "$HUB_BOX/sudo.log"
[ -f "$HUB_BOX/sudo_ok" ] || { echo 'sudo: a password is required' >&2; exit 1; }
[ "${1:-}" = -n ] && shift
HUB_SUDO=1 exec "$@"
'''
# ufw：开着、只放行了 22（Box.fake('ufw', UFW) 才有）
UFW = r'''[ "${1:-}" = status ] && printf 'Status: active\n\nTo                         Action      From\n--                         ------      ----\n22/tcp                     ALLOW       Anywhere\n'
'''
CHOWN = r'''printf '%s\n' "$*" >> "$HUB_BOX/chown.log"
'''
# ip -4 route get 地址：默认路由出去的源地址
IP = r'''echo "192.0.2.1 via 10.9.0.1 dev eth0 src $(cat "$HUB_BOX/ip") uid 0"
'''
JOURNAL = 'tail -n 20 "$HUB_BOX/server.log" 2>/dev/null\n'
# ss -ltn：listening 里的端口有程序在监听（Box.fake('ss', SS) 才有）
SS = r'''echo 'State  Recv-Q Send-Q Local Address:Port Peer Address:Port'
while read -r p; do echo "LISTEN 0 4096 0.0.0.0:$p 0.0.0.0:*"; done < "$HUB_BOX/listening"
'''
# 老的 docker-compose 1.x：装着也不该用它（no_compose 的用例查它的记录是空的）
COMPOSE_V1 = r'''printf '%s\n' "$*" >> "$HUB_BOX/docker-compose.log"
echo 'docker-compose version 1.29.2, build unknown'
'''
# 起服务端、停服务端（systemctl、docker 共用）：进程号记在 server.pid（自己的同步服务器，服务名 cca-hub），Team 的各有各的
# （server.<服务名>.pid：一台服务器上能装几个 Team）
SERVE = r'''
import os, re, signal, subprocess, sys, time
box = os.environ['HUB_BOX']
flag = lambda n: os.path.exists(os.path.join(box, n))

def pidf_of(svc):
    return os.path.join(box, 'server.pid' if svc == 'cca-hub' else 'server.%s.pid' % svc)

def running(svc='cca-hub'):
    try:
        pid = int(open(pidf_of(svc)).read())
        os.kill(pid, 0)
        return pid
    except (OSError, ValueError):
        return None

def stop(svc='cca-hub'):
    pid = running(svc)
    if pid:
        os.killpg(pid, signal.SIGTERM)
        for _ in range(200):
            if running(svc) is None:
                break
            try:
                if os.waitpid(pid, os.WNOHANG)[0]:
                    break
            except ChildProcessError:
                pass
            time.sleep(0.02)
    try:
        os.unlink(pidf_of(svc))
    except FileNotFoundError:
        pass

def start(argv, env, svc='cca-hub'):
    stop(svc)
    with open(os.path.join(box, 'server.log'), 'a') as log, open(os.path.join(box, 'server.argv'), 'a') as a:
        a.write(' '.join(argv) + '\n')
        p = subprocess.Popen(argv, env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True)
    open(pidf_of(svc), 'w').write(str(p.pid))

def env_file(path):
    return dict(l.split('=', 1) for l in open(path).read().splitlines() if '=' in l and not l.startswith('#'))
'''
SYSTEMCTL = SERVE + r'''
root = os.environ['CCA_HUB_ROOT']
a = sys.argv[1:]
open(os.path.join(box, 'systemctl.log'), 'a').write(' '.join(a) + '\n')
svc = (a[-1] if len(a) > 1 else 'cca-hub').replace('.service', '')   # 命令的最后一个参数是服务名（daemon-reload 没有：用不到）
unit = root + '/etc/systemd/system/%s.service' % svc
if a[:1] in (['restart'], ['start']):
    if flag('start_hang'):
        time.sleep(60)
    if flag('start_fail'):
        stop(svc)
        sys.exit('Job for %s.service failed because the control process exited with error code.' % svc)
    lines = open(unit).read().splitlines()
    kv = dict(l.split('=', 1) for l in lines if '=' in l and not l.startswith('#'))
    env = dict(os.environ, **env_file(root + kv['EnvironmentFile']))
    argv = kv['ExecStart'].split()
    argv = argv[:1] + [root + x if x.startswith('/') else x for x in argv[1:]]
    argv = [re.sub(r'\$\{(\w+)\}', lambda m: env[m.group(1)], x).replace('0.0.0.0', '127.0.0.1') for x in argv]
    start(argv, env, svc)
elif a[:2] == ['disable', '--now'] or a[:1] == ['stop']:
    stop(svc)
elif a[:2] == ['is-active', '--quiet']:
    sys.exit(0 if running(svc) else 3)
'''
DOCKER = SERVE + r'''
import json
a = sys.argv[1:]
open(os.path.join(box, 'docker.log'), 'a').write(' '.join(a) + '\n')
images = os.path.join(box, 'images')
if a[:1] == ['compose']:
    if flag('no_compose'):
        sys.exit("docker: 'compose' is not a docker command.")
    if a[1:2] == ['-p']:   # Team 的：项目名照服务名（两个 Team 的目录同名时不互相当成孤儿删掉）。记下来给测试看
        open(os.path.join(box, 'compose.projects'), 'a').write(a[2] + '\n')
        a = a[:1] + a[3:]
    if a[1:2] == ['-f']:   # 安装脚本总是写 -f compose.yaml（不认目录里别的 compose 文件）
        assert a[2] == 'compose.yaml', a
        a = a[:1] + a[3:]
if a[:2] == ['compose', 'version']:
    print('Docker Compose version v2.29.7')
elif a == ['info']:
    sys.exit(1 if flag('docker_down') else 0)
elif a[:1] == ['pull']:
    if flag('no_pull'):
        sys.exit(1)
    open(images, 'a').write(a[-1] + '\n')
elif a[:2] == ['image', 'inspect']:
    sys.exit(0 if flag('images') and a[2] in open(images).read().split() else 1)
elif a[:1] == ['inspect']:   # docker inspect -f 格式 容器名
    print('true' if running(a[-1]) else 'false')
elif a[:2] == ['compose', 'up']:
    if flag('compose_fail'):
        sys.exit('Error response from daemon: driver failed programming external connectivity on endpoint cca-hub')
    y, env = open('compose.yaml').read(), env_file('.env')
    svc = re.search(r'^    container_name: (\S+)$', y, re.M).group(1)
    # 交给服务端的那一项（入伙口令的哈希，Team 的是根公钥）：compose.yaml 里写着「名字: ${名字:?}」，值在 .env 里
    secret = re.search(r'^      (\w+): \$\{(\w+):\?\}$', y, re.M)
    assert secret and secret.group(1) == secret.group(2) and env['CCA_HUB_IMAGE'] in open(images).read().split()
    cmd = json.loads(re.search(r'^    command: (\[.*\])$', y, re.M).group(1))
    vols = dict((c, os.path.abspath(h)) for h, c in re.findall(r'^      - (\./\w+):(/\w+):', y, re.M))
    argv = [sys.executable] + [next((h + x[len(c):] for c, h in vols.items() if x.startswith(c + '/')), x) for x in cmd[1:]]
    argv = [x.replace('0.0.0.0:18445', '127.0.0.1:' + env['CCA_HUB_PORT']) for x in argv]
    start(argv, dict(os.environ, **{secret.group(1): env[secret.group(1)]}), svc)
elif a[:2] == ['compose', 'down']:
    y = open('compose.yaml').read() if os.path.exists('compose.yaml') else ''
    m = re.search(r'^    container_name: (\S+)$', y, re.M)
    stop(m.group(1) if m else 'cca-hub')
elif a[:2] == ['compose', 'logs']:
    sys.stdout.write(open(os.path.join(box, 'server.log')).read() if os.path.exists(os.path.join(box, 'server.log')) else '')
'''
MISSING = '#!/bin/sh\necho "$0: 这台假服务器上没有" >&2\nexit 127\n'


def free_port():
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]


class Box:
    """一台假的服务器：root/ 当作 /，bin/ 里是假的系统命令（sh 是 SH），enter 在它上面运行一条命令"""
    def __init__(self, d, docker=False, systemd=True, ip='203.0.113.7'):
        d = Path(d)
        self.d, self.root, self.bin = d, d / 'root', d / 'bin'
        for p in (self.root, self.bin, d / 'users'):
            p.mkdir(parents=True)
        if systemd:
            for p in ('run/systemd/system', 'etc/systemd/system'):
                (self.root / p).mkdir(parents=True)
        for name, script in (('id', ID), ('useradd', USERADD), ('userdel', USERDEL), ('chown', CHOWN), ('ip', IP),
                             ('journalctl', JOURNAL), ('sudo', SUDO), ('docker-compose', COMPOSE_V1)):
            self.fake(name, '#!/bin/sh\n' + script)
        self.fake('systemctl', '#!%s\n%s' % (sys.executable, SYSTEMCTL) if systemd else MISSING)
        self.fake('docker', '#!%s\n%s' % (sys.executable, DOCKER) if docker else MISSING)
        os.symlink(SH, self.bin / 'sh')
        (d / 'ip').write_text(ip)
        self.dir = self.root / 'opt' / 'cca-hub'
        self.env = {'PATH': '%s:/usr/bin:/bin' % self.bin, 'CCA_HUB_ROOT': str(self.root), 'HUB_BOX': str(d), 'HOME': str(d),
                    'LANG': 'en_US.UTF-8'}
        env = ' '.join("%s='%s'" % kv for kv in self.env.items())   # 都是这个目录下的路径：没有单引号
        self.enter = d / 'enter'
        self.enter.write_text('#!/bin/sh\n'
                              'if [ -f "%s/ssh_denied" ]; then\n'
                              '  [ "$(cat "%s/ssh_denied")" != once ] || rm -f "%s/ssh_denied"\n'
                              '  echo "Permission denied (publickey)." >&2; exit 255\n'
                              'fi\n'
                              'exec env -i %s /bin/sh -c "$1"\n' % (d, d, d, env))
        self.enter.chmod(0o755)

    def fake(self, name, text):
        p = self.bin / name
        p.write_text(text)
        p.chmod(0o755)

    def flag(self, name, on=True):
        p = self.d / name
        if on:
            p.touch()
        elif p.exists():
            p.unlink()

    def install(self, script, *args, shell=SH):
        # 新的会话：没有控制终端（像 cron、托管平台里那样），问不了公网地址
        return subprocess.run([shell, str(script)] + list(args), env=self.env, stdin=subprocess.DEVNULL, capture_output=True, text=True,
                              timeout=180, start_new_session=True)

    def log(self, name):
        p = self.d / name
        return p.read_text() if p.exists() else ''

    def envfile(self, d=None):
        f = (d or self.dir) / '.env'
        return dict(l.split('=', 1) for l in f.read_text().splitlines() if '=' in l and not l.startswith('#'))

    def pidfile(self, svc='cca-hub'):
        """这个服务的进程号文件（自己的同步服务器是 cca-hub；Team 的是 cca-team-<编号的前 8 位>）"""
        return self.d / ('server.pid' if svc == 'cca-hub' else 'server.%s.pid' % svc)

    def running(self, svc='cca-hub'):
        try:
            os.kill(int(self.pidfile(svc).read_text()), 0)
            return True
        except (OSError, ValueError):
            return False

    def stop(self, svc=None):
        """停掉这个服务；不写是哪个就都停"""
        for f in ([self.pidfile(svc)] if svc else list(self.d.glob('server*.pid'))):
            try:
                os.killpg(int(f.read_text()), 15)
            except (OSError, ValueError):
                pass

    def files_text(self):
        """服务器上（root/ 下）所有文件的内容，接在一起（找口令、盐有没有落盘）"""
        out = []
        for p in self.root.rglob('*'):
            if p.is_file() and not p.is_symlink():
                out.append(p.read_bytes().decode('utf-8', 'replace'))
        return '\n'.join(out)


if __name__ == '__main__':   # regress 11b3c：在这个目录建一台假服务器（cca hub deploy 经假 ssh 部署到它上面）
    Box(sys.argv[1])
