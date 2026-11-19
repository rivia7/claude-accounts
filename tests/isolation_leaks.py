#!/usr/bin/env python3
"""泄露检测的证据边界：临时 HOME、cca 的辅助程序（cca-core leak-verdict）、模拟探针，不联网。

只能在进程内测的一处在辅助程序自己的单元测试里（cargo test）：STUN 连不上、不回答时拿不到 IP
（cli/src/cmd/leak/stun.rs 的 failures_are_empty_never_an_address）。"""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from helper_bin import HELPER

LEAK_FAIL_TTL = 600   # 没测成的过 10 分钟再测（cli/src/cmd/leak.rs 的同名常量）
EXIT_IP = '203.0.113.10'
RESOLVER = '203.0.113.53'
KEY = 'router|US-A'


class IsolationLeaks(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='cca-leaks-')
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        (self.home / '.cca').mkdir()
        # 查归属地的 curl 一律拿不到（退出码 7）：结论只能来自探测结果里带着的
        (self.home / 'bin').mkdir()
        (self.home / 'bin' / 'curl').write_text('#!/bin/sh\necho "$@" >> "$HOME/curl.log"\nexit 7\n')
        (self.home / 'bin' / 'curl').chmod(0o755)
        self.env = {'HOME': str(self.home), 'CCA_HOME': str(self.home / '.cca'), 'CCA_TOOL': 'claude',
                    'PATH': '%s:/usr/bin:/bin' % (self.home / 'bin')}

    @staticmethod
    def ip_prefix(ip):
        """归属地按网段记：IPv4 的 /24（辅助程序的 ip_prefix）"""
        return '.'.join(ip.split('.')[:3]) + '.0/24'

    def geo(self, cc='us'):
        return {'t': time.time(), 'cc': cc, 'country': {'us': 'United States', 'cn': 'China', 'ca': 'Canada'}.get(cc, ''),
                'isp': 'Example ISP'}

    def dns(self, cc='us', servers=None):
        servers = [RESOLVER] if servers is None else servers
        return {'ok': True, 'servers': servers,
                'geo': {self.ip_prefix(ip): self.geo(cc) for ip in servers}}

    def verdict(self, payload):
        path = self.home / 'probe.json'
        path.write_text(json.dumps(payload))
        r = subprocess.run([HELPER, 'leak-verdict', str(path), KEY, EXIT_IP, 'US', 'detail'], env=self.env,
                           stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=30)
        self.assertEqual(r.returncode, 0, r.stderr)
        text = r.stdout
        states = {row.split('\t')[1]: row.split('\t')[2] for row in text.splitlines() if row.startswith('S\t')}
        return text, states

    def test_empty_or_missing_stun_result_is_failed(self):
        for payload in ({'dns': self.dns(), 'stun': {'ips': []}}, {'dns': self.dns()}, {}):
            with self.subTest(payload=payload):
                text, states = self.verdict(payload)
                self.assertEqual(states['webrtc'], 'failed')
                self.assertIn('STUN 探测未取得 IP', text)
                self.assertNotIn('无泄露', text)

    def test_webrtc_off_on_servers_is_not_mentioned(self):
        # 服务器上（没有账号专属的浏览器和桌面 App）leak-probe 写明 webrtc: off：不测也不说，不是“没测成”
        text, states = self.verdict({'dns': self.dns(), 'webrtc': 'off'})
        self.assertNotIn('webrtc', states)
        self.assertEqual(states['dns'], 'ok')
        self.assertNotIn('WebRTC', text)

    def test_empty_dns_response_is_failed_and_cached_briefly(self):
        text, states = self.verdict({'dns': self.dns(servers=[]), 'stun': {'ips': []}})
        self.assertEqual(states['dns'], 'failed')
        self.assertIn('未取得解析器', text)
        cached = json.loads((self.home / '.cca' / 'leak.json').read_text())['dns'][KEY]
        self.assertFalse(cached['ok'])
        # 没测成的只记 10 分钟：刚记下的还新（时间就是现在），过了 LEAK_FAIL_TTL 就不算数（leak_dns_fresh 的边界在
        # cli/src/cmd/leak.rs 的单元测试 freshness 里）
        self.assertLess(abs(time.time() - cached['t']), 30)
        self.assertEqual(LEAK_FAIL_TTL, 600)

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

    def stun(self, *where):
        dns = self.dns()
        for ip, cc in where:
            dns['geo'][self.ip_prefix(ip)] = self.geo(cc)
        return self.verdict({'dns': dns, 'stun': {'ips': [ip for ip, _ in where]}})

    def test_mainland_stun_ip_is_risk_not_browser_measurement(self):
        text, states = self.stun(('192.0.2.77', 'cn'))
        self.assertEqual(states['webrtc'], 'leak')
        self.assertIn('W\twebrtc\t', text)
        self.assertIn('WebRTC 直连风险：192.0.2.77', text)
        self.assertIn('尚未验证浏览器或 Claude App 是否会暴露该 IP', text)
        self.assertNotIn('都拿得到', text)
        self.assertNotIn('会暴露 192.0.2.77', text)

    def test_other_regions_are_only_a_note(self):
        for cc in ('us', 'hk', 'mo', 'tw'):
            with self.subTest(cc=cc):
                text, states = self.stun(('192.0.2.77', cc))
                self.assertEqual(states['webrtc'], 'note')
                self.assertNotIn('W\twebrtc', text)
                self.assertIn('N\twebrtc\t', text)
                self.assertIn('WebRTC 备注：STUN 看到 192.0.2.77', text)
                self.assertNotIn('直连风险', text)

    def test_any_mainland_address_is_a_risk(self):
        text, states = self.stun(('192.0.2.77', 'us'), ('198.51.100.77', 'cn'))
        self.assertEqual(states['webrtc'], 'leak')
        self.assertIn('WebRTC 直连风险：198.51.100.77', text)

    def test_unknown_location_is_neither_a_warning_nor_clear(self):
        text, states = self.verdict({'dns': self.dns(), 'stun': {'ips': ['192.0.2.77']}})
        self.assertEqual(states['webrtc'], 'failed')
        self.assertNotIn('W\twebrtc', text)
        self.assertIn('归属地没查到', text)

    def test_stun_without_ip_cannot_produce_ok(self):
        # STUN 连不上、不回答时探测结果里没有 IP（辅助程序的单元测试）；没有 IP 的结果不能判成通过
        _, states = self.verdict({'stun': {'ips': []}})
        self.assertEqual(states['webrtc'], 'failed')

    def test_dns_polling_without_observed_resolvers_does_not_succeed(self):
        # 查 DNS 的服务一直说没看到解析器（假 curl 每次都回空的列表）：问满几轮以后是没测成，不是通过
        (self.home / 'bin' / 'curl').write_text('''#!/bin/sh
echo "$@" >> "$HOME/curl.log"
case "$*" in */api/dns/result/*) echo '{"dns_servers": []}' ;; esac
exit 0
''')
        r = subprocess.run([HELPER, 'leak-probe', 'http://127.0.0.1:7901', KEY, '', '0'], env=self.env,
                           stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=30)
        self.assertEqual(r.returncode, 0, r.stderr)
        result = json.loads(r.stdout)
        self.assertEqual((result['dns']['ok'], result['dns']['servers']), (False, []))
        self.assertGreaterEqual((self.home / 'curl.log').read_text().count('/api/dns/result/'), 2)

    def test_disabled_check_emits_no_claim(self):
        text, states = self.verdict({'off': True})
        self.assertEqual(text, '')
        self.assertEqual(states, {})


if __name__ == '__main__':
    unittest.main()
