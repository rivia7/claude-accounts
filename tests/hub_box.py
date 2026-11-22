#!/usr/bin/env python3
"""一台假的 Linux 服务器，给 Team 的同步服务器的一键部署用（tests/hub_deploy.py；regress 11b3d 的 hubbox：python3 hub_box.py 目录）。
目录下的 root/ 当作 /（安装脚本的 CCA_HUB_ROOT），bin/ 里是假的系统命令：id、useradd、userdel、chown、ip、journalctl、sudo，
Python 写的 systemctl、docker（照 unit 的 ExecStart、compose.yaml 里各个服务的 command、端口和挂载起真的 server/cca-presence.py，
监听 127.0.0.1；证书助手 cca-acme.py 不真的去找 Let's Encrypt：换成 acme_stub.py——几个 Team 共用的那一个照 --teams 里登记的地址
一个地址写一张自签的证书、放进各个 Team 的目录，以前每个 Team 自己的那种照 --id 往 --out 里写一张；写过的记在 acme.issued），
sh 是 /bin/dash（没有时 /bin/sh）。没有的 docker、systemctl 也放一个会失败的：不然会找到这台机器上真的。
enter 是假 ssh 的服务器那一头：在这台假服务器上运行一条命令（stdin 照样给它；目录下有 ssh_denied 时登录不了，它的内容是 once 时只拒这一次）。
目录下这些文件在时：uid（id -u 说的；root 是 0）、sudo_ok（免密码的 sudo）、ip（默认路由出去的源地址）、listening（ss 说这些端口有人监听）、
no_pull（docker pull 拉不到）、docker_down（docker info 不通）、no_compose（没有 compose 插件，只有老的 docker-compose 1.x）、
compose_fail（docker compose up 起不来）、start_fail（systemctl restart 起不来）、start_hang（systemctl restart 一直不回来）、
acme_fail（证书助手要不到证书）。安装脚本等证书最多 CCA_HUB_CERT_WAIT 秒（这里是 4）；any_ip=True 时内网地址也开网页端（CCA_HUB_ANY_IP）。"""
import os
from pathlib import Path
import socket
import subprocess
import sys

SH = '/bin/dash' if os.path.exists('/bin/dash') else '/bin/sh'   # 假服务器上的 sh

ID = r'''case "${1:-}" in
	-u) if [ -n "${HUB_SUDO:-}" ]; then echo 0; else cat "$HUB_BOX/uid" 2>/dev/null || echo 0; fi ;;
	-g) cat "$HUB_BOX/users/$2" 2>/dev/null ;;
	'') echo 'uid=0(root) gid=0(root) groups=0(root)' ;;
	*) [ -f "$HUB_BOX/users/$1" ] ;;
esac
'''
# useradd：系统用户的文件里记着它的组（id -g 说的；第几个建的，4201 起）
USERADD = r'''printf '%s\n' "$*" >> "$HUB_BOX/useradd.log"
for a in "$@"; do u=$a; done
echo $((4201 + $(ls "$HUB_BOX/users" | wc -l))) > "$HUB_BOX/users/$u"
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
# 起服务端、停服务端（systemctl、docker 共用）：进程号记在 server.<服务名>.pid（一台服务器上能装几个 Team；证书助手的服务名是
# <服务名>-acme）。以前的自己的同步服务器（服务名 cca-hub）是 server.pid
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
    acme = [i for i, x in enumerate(argv) if x.endswith('/cca-acme.py')]
    if acme:   # 证书助手：不真的去找 Let's Encrypt，换成假的（照 --id 写一张自签的证书；acme_fail 在时什么都不写）
        argv = [sys.executable, os.path.join(box, 'acme_stub.py')] + argv[acme[0] + 1:]
    with open(os.path.join(box, 'server.log'), 'a') as log, open(os.path.join(box, 'server.argv'), 'a') as a:
        a.write(' '.join(argv) + '\n')
        p = subprocess.Popen(argv, env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True)
    open(pidf_of(svc), 'w').write(str(p.pid))

def env_file(path):
    return dict(l.split('=', 1) for l in open(path).read().splitlines() if '=' in l and not l.startswith('#'))
'''
# 假的证书助手：参数照真的，记进 acme.argv；不听端口、不联网，一直运行（像真的那样等着续）。几个 Team 共用的（--teams、--store、--out）：
# 照登记的地址一个地址一张自签的证书，存在 --store/<地址>/（手里没有、Team 的目录里有就接着用它：以前它自己的证书助手要的），放进
# --out/<服务名>/（目录不在不放：安装脚本建）；登记的变了马上照新的。以前每个 Team 自己的（--id、--out）：照 --id 写一张（有了不再写）。
# 写过的地址记在 acme.issued；acme_fail 在时什么都不写
ACME_STUB = r'''
import os, shutil, subprocess, sys, time
box = os.environ['HUB_BOX']
a = sys.argv[1:]
open(os.path.join(box, 'acme.argv'), 'a').write(' '.join(a) + '\n')
opt = lambda k: a[a.index(k) + 1]
assert os.path.isdir(opt('--account')), a
same = lambda x, y: os.path.exists(x) and os.path.exists(y) and open(x, 'rb').read() == open(y, 'rb').read()

def issue(d, cn):
    if os.path.exists(os.path.join(box, 'acme_fail')):
        return
    key, crt = os.path.join(d, 'privkey.pem'), os.path.join(d, 'fullchain.pem')
    subprocess.run(['openssl', 'ecparam', '-name', 'prime256v1', '-genkey', '-noout', '-out', key + '.tmp'], check=True, capture_output=True)
    subprocess.run(['openssl', 'req', '-new', '-x509', '-key', key + '.tmp', '-out', crt + '.tmp', '-days', '7', '-subj', '/CN=' + cn],
                   check=True, capture_output=True, stdin=subprocess.DEVNULL)
    os.replace(key + '.tmp', key)
    os.replace(crt + '.tmp', crt)
    open(os.path.join(box, 'acme.issued'), 'a').write(cn + '\n')

if '--id' in a:
    if not os.path.exists(os.path.join(opt('--out'), 'fullchain.pem')):
        issue(opt('--out'), opt('--id'))
    while True:
        time.sleep(3600)
teams, store, out = opt('--teams'), opt('--store'), opt('--out')
while True:
    for t in sorted(os.listdir(teams)):
        if '.' in t:
            continue   # 安装脚本正写到一半的
        ident = open(os.path.join(teams, t)).read().strip()
        s, d = os.path.join(store, ident.replace(':', '_')), os.path.join(out, t)
        os.makedirs(s, exist_ok=True)
        if not os.path.exists(os.path.join(s, 'fullchain.pem')):
            if os.path.exists(os.path.join(d, 'fullchain.pem')):
                for f in ('privkey.pem', 'fullchain.pem'):
                    shutil.copy(os.path.join(d, f), os.path.join(s, f))
            else:
                issue(s, ident)
        if os.path.isdir(d) and os.path.exists(os.path.join(s, 'fullchain.pem')):
            for f in ('privkey.pem', 'fullchain.pem'):
                if not same(os.path.join(s, f), os.path.join(d, f)):
                    shutil.copy(os.path.join(s, f), os.path.join(d, f + '.tmp'))
                    os.replace(os.path.join(d, f + '.tmp'), os.path.join(d, f))
    time.sleep(0.2)
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

def services(y):
    """compose.yaml 里各个服务的那一段（services: 下面缩进两格的名字开头）"""
    out = []
    for line in y.splitlines():
        if re.match(r'^  [\w.-]+:$', line):
            out.append([])
        elif out and line.startswith('    '):
            out[-1].append(line)
    return ['\n'.join(b) for b in out]

def interp(x, env):
    """compose 的 ${名字}、${名字:-默认}、${名字:?说明}"""
    def one(m):
        if m.group(2) == '-':
            return env.get(m.group(1)) or m.group(3)
        return env[m.group(1)]
    return re.sub(r'\$\{(\w+)(?::([-?])([^}]*))?\}', one, x)

a = sys.argv[1:]
open(os.path.join(box, 'docker.log'), 'a').write(' '.join(a) + '\n')
images = os.path.join(box, 'images')
proj = os.path.basename(os.getcwd())   # compose 的项目名：没写 -p 时是目录名
if a[:1] == ['compose']:
    if flag('no_compose'):
        sys.exit("docker: 'compose' is not a docker command.")
    if a[1:2] == ['-p']:   # Team 的：项目名照服务名（两个 Team 的目录同名时不互相当成孤儿删掉）。记下来给测试看
        open(os.path.join(box, 'compose.projects'), 'a').write(a[2] + '\n')
        proj = a[2]
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
        sys.exit('Error response from daemon: driver failed programming external connectivity on endpoint cca-team')
    y, env = open('compose.yaml').read(), env_file('.env')
    assert env['CCA_HUB_IMAGE'] in open(images).read().split()
    # 这个项目上次起的容器（compose.<项目名>.up）：--remove-orphans 时 compose.yaml 里已经没有的停掉（网页端关了，证书助手就没了）
    up = os.path.join(box, 'compose.%s.up' % proj)
    before = open(up).read().split() if os.path.exists(up) else []
    names = []
    for block in services(y):
        svc = re.search(r'^    container_name: (\S+)$', block, re.M).group(1)
        names.append(svc)
        # 交给容器的环境变量（Team 的根公钥）：compose.yaml 里写着「名字: ${名字:?}」，值在 .env 里
        given = {k: env[v] for k, v in re.findall(r'^      (\w+): \$\{(\w+):\?\}$', block, re.M)}
        cmd = json.loads(re.search(r'^    command: (\[.*\])$', block, re.M).group(1))
        # 挂载：./目录 是 compose.yaml 旁边的，/绝对路径 是这台假服务器上的（root/ 下面）
        vols = dict((c, os.path.abspath(h) if h.startswith('./') else os.environ['CCA_HUB_ROOT'] + h)
                    for h, c in re.findall(r'^      - (\./\w+|/[\w./-]+):(/\w+)(?::|$)', block, re.M))
        argv = [sys.executable] + [next((h + x[len(c):] for c, h in vols.items() if x == c or x.startswith(c + '/')), x) for x in cmd[1:]]
        # compose 做的两件事：${名字:?} 换成 .env 里的值；容器里的端口经 ports 那几行对到外面的
        argv = [interp(x, env) for x in argv]
        ports = {c: interp(h, env) for h, c in re.findall(r'^      - "([^"]+):(\d+)"$', block, re.M)}
        argv = [re.sub(r'^0\.0\.0\.0:(\d+)$', lambda m: '127.0.0.1:' + ports.get(m.group(1), '0'), x) for x in argv]
        if 'cca-presence.py' in ' '.join(argv):
            assert ('--relay-listen' in argv) == (env.get('CCA_HUB_RELAY', 'off') != 'off') == ('18446' in ports), \
                '开着中转：命令里有那两个参数、ports 里有数据端口；没开：都没有'
            assert ('--web-listen' in argv) == ('CCA_HUB_WEB_URL' in env) == ('18447' in ports) == (':/webcert:ro,z' in block), \
                '开着网页端：命令里有那几个参数、ports 里有它的端口、只读挂着共用的证书助手放给它的证书；没开：都没有'
        else:
            assert ports == {'18080': '80'} and 'cca-acme.py' in ' '.join(argv), '证书助手：80 端口对到容器里的 18080'
        start(argv, dict(os.environ, **given), svc)
    for old in before:
        if old not in names and '--remove-orphans' in a:
            stop(old)
    open(up, 'w').write(' '.join(names))
elif a[:2] == ['compose', 'down']:
    y = open('compose.yaml').read() if os.path.exists('compose.yaml') else ''
    for block in services(y) or ['    container_name: cca-hub']:
        stop(re.search(r'^    container_name: (\S+)$', block, re.M).group(1))
    if os.path.exists(os.path.join(box, 'compose.%s.up' % proj)):
        os.unlink(os.path.join(box, 'compose.%s.up' % proj))
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
    def __init__(self, d, docker=False, systemd=True, ip='203.0.113.7', any_ip=False):
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
        (d / 'acme_stub.py').write_text(ACME_STUB)
        (d / 'ip').write_text(ip)
        self.dir = self.root / 'opt' / 'cca-hub'   # 以前的自己的同步服务器装在这里（卸载它的用例）
        self.env = {'PATH': '%s:/usr/bin:/bin' % self.bin, 'CCA_HUB_ROOT': str(self.root), 'HUB_BOX': str(d), 'HOME': str(d),
                    'LANG': 'en_US.UTF-8', 'CCA_HUB_CERT_WAIT': '4'}
        if any_ip:
            self.env['CCA_HUB_ANY_IP'] = '1'
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

    def install(self, script, *args, shell=SH, env=None):
        # 新的会话：没有控制终端（像 cron、托管平台里那样），问不了公网地址。env：另加的环境变量（cca team deploy 放进的那几项）
        return subprocess.run([shell, str(script)] + list(args), env=dict(self.env, **(env or {})), stdin=subprocess.DEVNULL,
                              capture_output=True, text=True, timeout=180, start_new_session=True)

    def log(self, name):
        p = self.d / name
        return p.read_text() if p.exists() else ''

    def envfile(self, d=None):
        f = (d or self.dir) / '.env'
        return dict(l.split('=', 1) for l in f.read_text().splitlines() if '=' in l and not l.startswith('#'))

    def pidfile(self, svc='cca-hub'):
        """这个服务的进程号文件（Team 的是 cca-team-<编号的前 8 位>，它的证书助手再加 -acme；以前的自己的同步服务器是 cca-hub）"""
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


if __name__ == '__main__':   # regress 11b3c：在这个目录建一台假服务器（cca team deploy 经假 ssh 部署到它上面）
    Box(sys.argv[1])
