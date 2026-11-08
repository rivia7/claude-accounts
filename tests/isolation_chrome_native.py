#!/usr/bin/env python3
"""可选真实 Chrome WebRTC 回归：只用临时 HOME、空 profile 和本机 HTTP/STUN。

运行：CCA_TEST_CHROME_BIN='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' \
          python3 tests/isolation_chrome_native.py
未设置变量时跳过。macOS 沙箱内启动 Chrome 可能需要单独授权。
这里验证 cca 的偏好清理和启动参数对受控 ICE 的效果，不验证硬件指纹，
也不覆盖企业策略、已安装扩展或浏览器的其他网络通道。不会保存候选 IP。
"""
import base64
import gzip
import http.server
import json
import os
from pathlib import Path
import re
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import unittest


SCRIPT = (Path(__file__).resolve().parents[1] / 'cca').read_text()
SOURCE = SCRIPT.split("PY_SRC=$(cat <<'PY'\n", 1)[1].split('\nPY\n)', 1)[0]
PY_BOOT = SCRIPT.split("PY_BOOT='", 1)[1].split("'\n", 1)[0]
SOURCE_PACKED = base64.b64encode(gzip.compress(SOURCE.encode(), mtime=0)).decode()
CHROME_OPEN = re.search(r'^chrome_open_url\(\) \{\n.*?^\}', SCRIPT, re.M | re.S).group()
CHROME = os.environ.get('CCA_TEST_CHROME_BIN', '')
ACCOUNT = 'native-browser@example.com'
PROXY = 'http://127.0.0.1:9'


class LoopbackIceProbe:
    """只记录 UDP 次数和候选类型；STUN 映射地址是文档专用假 IP。"""

    def __enter__(self):
        self.done = threading.Event()
        self.stop = threading.Event()
        self.lock = threading.Lock()
        self.packets = 0
        self.result = None
        self.udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.udp.bind(('127.0.0.1', 0))
        self.udp.settimeout(0.2)
        probe = self
        page = '''<!doctype html><meta charset="utf-8"><script>
(async () => {
  const result = {candidates: []};
  const peer = new RTCPeerConnection({iceServers: [{urls: 'stun:127.0.0.1:PORT'}]});
  let reported = false;
  async function report() {
    if (reported) return;
    reported = true;
    result.state = peer.iceGatheringState;
    peer.close();
    await fetch('/result', {method: 'POST', body: JSON.stringify(result)});
  }
  peer.onicecandidate = event => {
    if (event.candidate) {
      const candidate = event.candidate;
      result.candidates.push({type: candidate.type, protocol: candidate.protocol,
                             testAddress: candidate.address === '198.51.100.77'});
    } else report();
  };
  peer.createDataChannel('probe');
  await peer.setLocalDescription(await peer.createOffer());
  setTimeout(report, 3500);
})().catch(() => fetch('/result', {method: 'POST', body: '{"error":true}'}));
</script>'''.replace('PORT', str(self.udp.getsockname()[1])).encode()

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def do_GET(self):
                self.send_response(200)
                self.send_header('Content-Type', 'text/html; charset=utf-8')
                self.send_header('Content-Length', str(len(page)))
                self.end_headers()
                self.wfile.write(page)

            def do_POST(self):
                length = int(self.headers.get('Content-Length', 0))
                if self.path != '/result' or not 0 < length < 8192:
                    self.send_error(400)
                    return
                result = json.loads(self.rfile.read(length))
                with probe.lock:
                    probe.result = result
                self.send_response(204)
                self.end_headers()
                probe.done.set()

        self.http = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.threads = [threading.Thread(target=self.stun, daemon=True),
                        threading.Thread(target=self.http.serve_forever, daemon=True)]
        for thread in self.threads:
            thread.start()
        self.url = 'http://127.0.0.1:%d/' % self.http.server_address[1]
        return self

    def stun(self):
        cookie = 0x2112A442
        address = bytes(a ^ b for a, b in zip(socket.inet_aton('198.51.100.77'), struct.pack('!I', cookie)))
        mapped = struct.pack('!BBH4s', 0, 1, 54321 ^ (cookie >> 16), address)
        attribute = struct.pack('!HH', 0x0020, len(mapped)) + mapped
        while not self.stop.is_set():
            try:
                request, peer = self.udp.recvfrom(2048)
            except socket.timeout:
                continue
            except OSError:
                return
            if len(request) < 20 or request[:2] != b'\x00\x01' or request[4:8] != struct.pack('!I', cookie):
                continue
            with self.lock:
                self.packets += 1
            response = struct.pack('!HHI', 0x0101, len(attribute), cookie) + request[8:20] + attribute
            try:
                self.udp.sendto(response, peer)
            except OSError:
                return

    def __exit__(self, *_args):
        self.stop.set()
        self.udp.close()
        self.http.shutdown()
        self.http.server_close()
        for thread in self.threads:
            thread.join(timeout=3)


@unittest.skipUnless(CHROME, '设置 CCA_TEST_CHROME_BIN 后才运行真实 Chrome（仅本机服务）')
class NativeChromeIsolation(unittest.TestCase):
    def setUp(self):
        self.assertTrue(Path(CHROME).is_absolute(), 'CCA_TEST_CHROME_BIN 必须是绝对路径')
        self.assertTrue(os.access(CHROME, os.X_OK), 'CCA_TEST_CHROME_BIN 不是可执行文件')
        self.temp = tempfile.TemporaryDirectory(prefix='cca-chrome-native-')
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        self.env = {'HOME': str(self.home), 'TMPDIR': str(self.home), 'PATH': '/usr/bin:/bin:/usr/sbin:/sbin',
                    'CCA_POLICY_DEFAULT': ''}

    def helper(self, tool, *args):
        result = subprocess.run(
            [sys.executable, '-I', '-X', 'utf8', '-c', PY_BOOT, '', SOURCE_PACKED, *args],
            env=dict(self.env, CCA_TOOL=tool), capture_output=True, text=True, timeout=15)
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout.strip()

    def launch_args(self, profile, url):
        # 执行真正的 shell 函数，只替换 open 来捕获参数，不调用 LaunchServices。
        shell = 'open() { printf "%s\\0" "$@"; }\n' + CHROME_OPEN + '\nchrome_open_url "$1" "$2"\n'
        result = subprocess.run(
            ['/bin/bash', '-c', shell, 'cca-chrome-args', CHROME, url],
            env=dict(self.env, ACC_CHROME_DIR=str(profile), ACC_PROXY=PROXY),
            capture_output=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        arguments = result.stdout.decode().rstrip('\0').split('\0')
        self.assertEqual(arguments[:4], ['-n', '-a', CHROME, '--args'])
        arguments = arguments[4:]
        self.assertIn('--webrtc-ip-handling-policy=disable_non_proxied_udp', arguments)
        self.assertIn('--user-data-dir=' + str(profile), arguments)
        self.assertIn('--proxy-server=' + PROXY, arguments)
        return arguments

    def ice(self, profile):
        with LoopbackIceProbe() as probe:
            arguments = self.launch_args(profile, probe.url)
            # 实际 cca 参数保持不变；以下只关闭测试无关后台功能、使用假 keychain，
            # 并允许本机 HTTP/STUN。外部 HTTP 被送到未监听的 127.0.0.1:9。
            testing = [
                '--headless=new', '--disable-background-networking', '--disable-component-update',
                '--disable-default-apps', '--disable-extensions', '--disable-sync', '--disable-breakpad',
                '--disable-crash-reporter', '--disable-crashpad-for-testing', '--disable-updater-scheduler',
                '--metrics-recording-only', '--password-store=basic', '--use-mock-keychain',
                '--allow-loopback-in-peer-connection', '--proxy-bypass-list=127.0.0.1;localhost',
                '--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1, EXCLUDE localhost',
            ]
            child = subprocess.Popen(
                [CHROME, *testing, *arguments], env=self.env, start_new_session=True,
                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            try:
                self.assertTrue(probe.done.wait(15), 'Chrome 未完成本机 ICE；不能把空结果当作隔离成功')
                with probe.lock:
                    result, packets = probe.result, probe.packets
                self.assertIsInstance(result, dict)
                self.assertNotIn('error', result, '本机 WebRTC 测试页面执行失败')
                self.assertEqual(result.get('state'), 'complete', 'ICE 未完成；不能据此判定 UDP 已阻断')
                return packets, result.get('candidates', [])
            finally:
                # Chrome 的子进程也属于此进程组；包括启动失败/超时路径都必须收尾。
                try:
                    os.killpg(child.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    pass
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                child.wait(timeout=5)

    def test_prepare_removes_url_exception_and_real_chrome_blocks_udp(self):
        for tool in ('claude', 'codex'):
            with self.subTest(tool=tool):
                profile = Path(self.helper(tool, 'private-profile', ACCOUNT, 'browser'))
                self.assertIn(self.home.resolve(), profile.resolve().parents, '浏览器资料目录必须位于测试临时 HOME')
                preferences = profile / 'Default' / 'Preferences'
                preferences.parent.mkdir(parents=True)
                contaminated = {'test_marker': 'preserve', 'webrtc': {
                    'ip_handling_policy': 'disable_non_proxied_udp',
                    'ip_handling_url': [{'url': '*', 'handling': 'default'}],
                    'local_ips_allowed_urls': ['*'],
                }}
                preferences.write_text(json.dumps(contaminated))
                baseline_packets, baseline_candidates = self.ice(profile)
                self.assertGreater(baseline_packets, 0, 'baseline 必须实际收到 STUN UDP；否则测试不能证明防护有效')
                self.assertTrue(any(c.get('type') == 'srflx' and c.get('testAddress')
                                    for c in baseline_candidates), 'baseline 未接收到受控 STUN 响应')

                # Chrome 完全退出后重新注入例外，再调用生产 helper 清理。
                preferences.write_text(json.dumps(contaminated))
                self.helper(tool, 'browser-profile', 'prepare', ACCOUNT)
                self.helper(tool, 'browser-profile', 'check', ACCOUNT)
                prepared = json.loads(preferences.read_text())
                self.assertEqual(prepared['test_marker'], 'preserve')
                self.assertEqual(prepared['webrtc']['ip_handling_policy'], 'disable_non_proxied_udp')
                self.assertEqual(prepared['webrtc']['ip_handling_url'], [])
                self.assertEqual(prepared['webrtc']['local_ips_allowed_urls'], [])
                packets, candidates = self.ice(profile)
                self.assertEqual(packets, 0, '准备后的账号 profile 仍向本机 STUN 发出了非代理 UDP')
                self.assertFalse(any(c.get('protocol') == 'udp' for c in candidates),
                                 '准备后的账号 profile 仍生成 UDP ICE candidate')


if __name__ == '__main__':
    unittest.main(verbosity=2)
