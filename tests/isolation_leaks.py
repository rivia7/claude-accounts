#!/usr/bin/env python3
"""泄露检测的证据边界：临时 HOME、内嵌 Python、模拟探针，不联网。"""
import contextlib
import io
import json
import os
from pathlib import Path
import socket
import tempfile
import time
import unittest
from unittest import mock


CCA = Path(__file__).resolve().parents[1] / 'cca'
SOURCE = CCA.read_text().split("PY_SRC=$(cat <<'PY'\n", 1)[1].split('\nPY\n)', 1)[0]
DEFINITIONS = SOURCE.split('\nif len(sys.argv) < 2', 1)[0]
EXIT_IP = '203.0.113.10'
RESOLVER = '203.0.113.53'
KEY = 'router|US-A'


class IsolationLeaks(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='cca-leaks-')
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        (self.home / '.cca').mkdir()
        with mock.patch.dict(os.environ, {'HOME': str(self.home), 'CCA_HOME': str(self.home / '.cca'),
                                          'CCA_TOOL': 'claude'}, clear=True):
            self.ns = {}
            exec(compile(DEFINITIONS, str(CCA), 'exec'), self.ns)
        self.ns['leak_curl'] = mock.Mock(return_value=None)

    def geo(self, cc='us'):
        return {'t': time.time(), 'cc': cc, 'country': {'us': 'United States', 'cn': 'China', 'ca': 'Canada'}.get(cc, ''),
                'isp': 'Example ISP'}

    def dns(self, cc='us', servers=None):
        servers = [RESOLVER] if servers is None else servers
        return {'ok': True, 'servers': servers,
                'geo': {self.ns['ip_prefix'](ip): self.geo(cc) for ip in servers}}

    def verdict(self, payload):
        path = self.home / 'probe.json'
        path.write_text(json.dumps(payload))
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.ns['cmd_leak_verdict'](str(path), KEY, EXIT_IP, 'US', 'detail')
        text = out.getvalue()
        states = {row.split('\t')[1]: row.split('\t')[2] for row in text.splitlines() if row.startswith('S\t')}
        return text, states

    def test_empty_or_missing_stun_result_is_failed(self):
        for payload in ({'dns': self.dns(), 'stun': {'ips': []}}, {'dns': self.dns()}, {}):
            with self.subTest(payload=payload):
                text, states = self.verdict(payload)
                self.assertEqual(states['webrtc'], 'failed')
                self.assertIn('STUN 探测未取得 IP', text)
                self.assertNotIn('无泄露', text)

    def test_empty_dns_response_is_failed_and_cached_briefly(self):
        text, states = self.verdict({'dns': self.dns(servers=[]), 'stun': {'ips': []}})
        self.assertEqual(states['dns'], 'failed')
        self.assertIn('未取得解析器', text)
        cached = json.loads(Path(self.ns['LEAK_FILE']).read_text())['dns'][KEY]
        self.assertFalse(cached['ok'])
        cached['t'] = time.time() - self.ns['LEAK_FAIL_TTL'] - 1
        self.assertFalse(self.ns['leak_dns_fresh'](cached))

    def test_missing_dns_geo_is_failed(self):
        for cc in ('', 'unknown', '??'):
            with self.subTest(cc=cc):
                text, states = self.verdict({'dns': self.dns(cc), 'stun': {'ips': [EXIT_IP]}})
                self.assertEqual(states['dns'], 'failed')
                self.assertIn('解析器归属地不完整', text)

    def test_known_foreign_resolver_only_establishes_region_check(self):
        text, states = self.verdict({'dns': self.dns('ca'), 'stun': {'ips': [EXIT_IP]}})
        self.assertEqual(states, {'dns': 'ok', 'webrtc': 'ok'})
        self.assertIn('DNS 未发现地区异常', text)
        self.assertIn('非浏览器实测', text)
        self.assertIn('STUN 出口一致（浏览器状态未验证）', text)
        self.assertNotIn('无泄露', text)

    def test_known_cn_resolver_is_still_reported_with_an_unknown_resolver(self):
        payload = self.dns('cn')
        payload['servers'].append('192.0.2.53')
        text, states = self.verdict({'dns': payload, 'stun': {'ips': [EXIT_IP]}})
        self.assertEqual(states['dns'], 'leak')
        self.assertIn('DNS 泄露（1 个解析器在中国大陆）', text)

    def test_different_host_stun_ip_is_risk_not_browser_measurement(self):
        text, states = self.verdict({'dns': self.dns(), 'stun': {'ips': ['192.0.2.77']}})
        self.assertEqual(states['webrtc'], 'leak')
        self.assertIn('WebRTC 直连风险：192.0.2.77', text)
        self.assertIn('尚未验证浏览器或 Claude App 是否会暴露该 IP', text)
        self.assertNotIn('都拿得到', text)
        self.assertNotIn('会暴露 192.0.2.77', text)

    def test_stun_dns_failure_or_timeout_cannot_produce_ok(self):
        with mock.patch.object(socket, 'getaddrinfo', side_effect=socket.gaierror('unavailable')):
            ip = self.ns['stun_probe']('stun.example.com', 3478, socket.AF_INET, 0.01)
        self.assertEqual(ip, '')
        fake_socket = mock.Mock()
        fake_socket.recv.side_effect = socket.timeout('timeout')
        with mock.patch.object(socket, 'getaddrinfo', return_value=[(socket.AF_INET, socket.SOCK_DGRAM, 0, '', ('192.0.2.1', 3478))]), \
                mock.patch.object(socket, 'socket', return_value=fake_socket):
            ip = self.ns['stun_probe']('stun.example.com', 3478, socket.AF_INET, 0.01)
        self.assertEqual(ip, '')
        fake_socket.close.assert_called_once()
        _, states = self.verdict({'stun': {'ips': [ip] if ip else []}})
        self.assertEqual(states['webrtc'], 'failed')

    def test_dns_polling_without_observed_resolvers_does_not_succeed(self):
        self.ns['leak_curl'] = mock.Mock(return_value=json.dumps({'dns_servers': []}))
        with mock.patch.object(time, 'sleep'):
            result = self.ns['leak_dns']('http://127.0.0.1:7901', time.time() + 5)
        self.assertEqual(result, {'ok': False, 'servers': []})

    def test_disabled_check_emits_no_claim(self):
        text, states = self.verdict({'off': True})
        self.assertEqual(text, '')
        self.assertEqual(states, {})


if __name__ == '__main__':
    unittest.main()
