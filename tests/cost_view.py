#!/usr/bin/env python3
"""折合金额（cost）：价格表、读会话记录（增量、去重、按账号身份认）、时段（本地日历日；额度窗口和日历周）、_gui state 的 cost、cost_total，
密文的格式（照 server/README.md 独立实现一遍核对）。临时 HOME、内嵌 Python，不联网。"""
import base64
import importlib.util
import json
import os
from pathlib import Path
import random
import sys
import tempfile
import time
import unittest
from unittest import mock
import zlib

ROOT = Path(__file__).resolve().parents[1]
CCA = ROOT / 'cca'
SOURCE = CCA.read_text().split("PY_SRC=$(cat <<'PY'\n", 1)[1].split('\nPY\n)', 1)[0]
DEFINITIONS = SOURCE.split('\nif len(sys.argv) < 2', 1)[0]

sys.dont_write_bytecode = True   # 不在 server/ 里留下 __pycache__
_spec = importlib.util.spec_from_file_location('cca_presence_server', ROOT / 'server' / 'cca-presence.py')
srvmod = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(srvmod)
import spec_wire as sw   # noqa: E402  照 server/README.md 的格式独立实现的密文（不用 cca 的代码）

U1 = 'aaaaaaaa-0000-4000-8000-000000000001'
U2 = 'aaaaaaaa-0000-4000-8000-000000000002'
U3 = 'user-' + 'c' * 24
U4 = 'user-' + 'd' * 24
M = 1000000   # 一美元 = 这么多个百万分之一美元


def jl(*rows):
    return ''.join(json.dumps(r, separators=(',', ':')) + '\n' for r in rows)


def iso(ts):
    return time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(ts)) + '.000Z'


class Base(unittest.TestCase):
    TZ = 'Asia/Shanghai'

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='cca-cost-')
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name).resolve()
        self.cca = self.home / '.cca'
        self.cca.mkdir()
        (self.cca / 'config.json').write_text('{}')
        old_tz = os.environ.get('TZ')
        os.environ['TZ'] = self.TZ
        time.tzset()

        def restore():
            if old_tz is None:
                os.environ.pop('TZ', None)
            else:
                os.environ['TZ'] = old_tz
            time.tzset()
        self.addCleanup(restore)
        with mock.patch.dict(os.environ, {'HOME': str(self.home), 'CCA_HOME': str(self.cca), 'CCA_TOOL': 'claude', 'TZ': self.TZ,
                                         'PATH': os.environ.get('PATH', '/usr/bin:/bin')}, clear=True):
            self.ns = {}
            exec(compile(DEFINITIONS, str(CCA), 'exec'), self.ns)
        self.ns['cost_kick'] = lambda: None


class Prices(Base):
    def test_model_names(self):
        f = self.ns['cost_model']
        for raw, want in (('claude-opus-5-5[1m]', 'claude-opus-5-5'), ('claude-sonnet-4-5-20250929', 'claude-sonnet-4-5'),
                          ('Claude-Haiku-4-5', 'claude-haiku-4-5'), ('claude-x-2026-01-15', 'claude-x'), ('gpt-6-astra', 'gpt-6-astra'),
                          (' gpt-6.1-sol ', 'gpt-6.1-sol'), (None, ''), (5, ''), ('m' * 100, 'm' * 64)):
            self.assertEqual(f(raw), want, raw)

    def test_claude(self):
        f = self.ns['cost_claude']
        u = {'input_tokens': 1000, 'output_tokens': 100, 'cache_read_input_tokens': 10000,
             'cache_creation': {'ephemeral_5m_input_tokens': 2000, 'ephemeral_1h_input_tokens': 500}}
        usd, tok = f('claude-opus-5-5', u)
        self.assertAlmostEqual(usd, 1000 * 4 + 2000 * 5 + 500 * 8 + 10000 * 0.2 + 100 * 20)
        self.assertEqual(tok, (1000, 100, 10000, 2500), '(输入, 输出, 缓存读, 缓存写)')
        self.assertAlmostEqual(f('claude-opus-5-5', dict(u, speed='fast'))[0], 1000 * 8 + 2000 * 10 + 500 * 16 + 10000 * 0.4 + 100 * 40,
                               msg='快速模式：输入、输出按快速模式的标价，缓存跟着输入翻倍')
        self.assertAlmostEqual(f('claude-sonnet-5-5', dict(u, speed='fast'))[0], f('claude-sonnet-5-5', u)[0], msg='没有快速模式标价的模型照常')
        self.assertAlmostEqual(f('claude-opus-5-5', dict(u, inference_geo='us'))[0], f('claude-opus-5-5', u)[0] * 1.1, msg='只在美国推理：加 10%')
        self.assertAlmostEqual(f('claude-sonnet-4-5', dict(u, inference_geo='us'))[0], 1000 * 3 + 2000 * 3.75 + 500 * 6 + 10000 * 0.3 + 100 * 15,
                               msg='4.6 以前的模型不加价')
        self.assertAlmostEqual(f('claude-opus-5-5', {'cache_creation_input_tokens': 1000})[0], 1000 * 5, msg='没有分 5 分钟、1 小时的：按 5 分钟算')
        self.assertAlmostEqual(f('claude-opus-5-5', {'output_tokens': 10, 'server_tool_use': {'web_search_requests': 3}})[0], 10 * 20 + 3 * 10000,
                               msg='网页搜索一次 0.01 美元')
        self.assertAlmostEqual(f('claude-haiku-5-5', {'input_tokens': 50000, 'output_tokens': 1000})[0], 50000 * 0.1 + 1000 * 0.5)
        self.assertAlmostEqual(f('claude-haiku-5-5', {'input_tokens': 90000, 'cache_read_input_tokens': 20000, 'output_tokens': 1000})[0],
                               90000 * 0.5 + 20000 * 0.05 + 1000 * 2.5, msg='输入（含缓存）超过 10 万 token：长上下文的标价')
        self.assertEqual(f('claude-unknown-9', u), (None, (1000, 100, 10000, 2500)), '没有公开价格的：只数 token')
        for junk in (True, -5, 1.5, '100', 10 ** 13, None):
            self.assertEqual(f('claude-opus-5-5', {'input_tokens': junk, 'output_tokens': 1})[1], (0, 1, 0, 0), junk)

    def test_codex(self):
        f = self.ns['cost_codex']
        u = {'input_tokens': 10000, 'cached_input_tokens': 8000, 'cache_write_input_tokens': 0, 'output_tokens': 500,
             'reasoning_output_tokens': 300}
        base = 2000 * 10 + 8000 * 1 + 500 * 50
        self.assertEqual(f('gpt-6-astra', 'default', u), (base, (2000, 500, 8000, 0)), '输入含缓存读；推理算在输出里')
        for tier, k in ((None, 1), ('priority', 2), ('fast', 2), ('flex', 0.5), ('ultrafast', 6)):
            self.assertAlmostEqual(f('gpt-6-astra', tier, u)[0], base * k, msg=tier)
        self.assertAlmostEqual(f('gpt-6-sol', 'ultrafast', u)[0], (2000 * 2 + 8000 * 0.2 + 500 * 10) * 2, msg='没有 Ultrafast 标价的模型按 Fast 算')
        self.assertAlmostEqual(f('gpt-6-astra', None, {'input_tokens': 300000})[0], 300000 * 20, msg='输入超过 272K：长上下文的标价')
        self.assertAlmostEqual(f('gpt-6-astra', None, {'input_tokens': 5000, 'cached_input_tokens': 1000, 'cache_write_input_tokens': 2000})[0],
                               2000 * 10 + 1000 * 1 + 2000 * 12.5)
        self.assertAlmostEqual(f('gpt-5.3-codex', None, {'input_tokens': 5000, 'cached_input_tokens': 1000, 'cache_write_input_tokens': 2000})[0],
                               2000 * 1.75 + 1000 * 0.175 + 2000 * 1.75, msg='没有缓存写标价的按输入算')
        self.assertAlmostEqual(f('gpt-5.3-codex', None, {'input_tokens': 300000})[0], 300000 * 1.75, msg='没有长上下文标价的照常')
        self.assertEqual(f('gpt-6-astra', None, {'input_tokens': 100, 'cached_input_tokens': 500})[1], (0, 0, 100, 0), '缓存不超过输入')
        self.assertEqual(f('codex-auto-review', None, u), (None, (2000, 500, 8000, 0)))


class Scan(Base):
    def setUp(self):
        super().setUp()
        self.t0 = (int(time.time()) // 3600 - 48) * 3600
        self.proj = self.home / '.claude' / 'projects' / '-work'
        self.proj.mkdir(parents=True)
        self.rollouts = self.home / '.codex' / 'sessions' / '2026' / '11' / '10'
        self.rollouts.mkdir(parents=True)

    def reply(self, ts, mid, sid='s1', model='claude-opus-5-5', inp=1000, out=100):
        return {'type': 'assistant', 'timestamp': iso(ts), 'requestId': 'req_' + mid, 'sessionId': sid,
                'message': {'id': mid, 'model': model, 'role': 'assistant', 'usage': {'input_tokens': inp, 'output_tokens': out}}}

    @staticmethod
    def owner(uuid, sid='s1'):
        return {'type': 'session-owner', 'accountUuid': uuid, 'organizationUuid': 'org-1', 'sessionId': sid}

    def codex(self, ts, kind, payload):
        return {'timestamp': iso(ts), 'type': kind, 'payload': payload}

    def tokens(self, ts, total, last):
        return self.codex(ts, 'event_msg', {'type': 'token_count', 'info': {'total_token_usage': total, 'last_token_usage': last},
                                            'rate_limits': {}})

    def scan(self):
        j = self.j if hasattr(self, 'j') else self.ns['cost_fresh']()
        self.j = j
        return self.ns['cost_scan'](j, time.time())

    def total(self, ident):
        rows = self.j['orphan'] if ident is None else self.j['h'].get(ident, {})
        return [sum(r[i] for r in rows.values()) for i in range(7)]

    def write_all(self):
        t0 = self.t0
        (self.proj / 's1.jsonl').write_text(jl(
            {'type': 'permission-mode', 'mode': 'default', 'sessionId': 's1'},
            self.owner(U1), self.reply(t0, 'm1'), self.reply(t0, 'm1'),   # 同一条回复的两块：只算一次
            {'type': 'assistant', 'timestamp': iso(t0), 'message': {'model': '<synthetic>', 'usage': {'input_tokens': 9}}},
            {'type': 'user', 'timestamp': iso(t0), 'message': {'content': 'mentions "usage" and "assistant" and "session-owner"'}},
            self.reply(t0 + 3600, 'm2', model='claude-unknown-1'),
            self.owner(U2),   # 换了账号续接：以后的回复是 U2 的
            self.reply(t0 + 7200, 'm3')))
        sub = self.proj / 's1' / 'subagents'
        sub.mkdir(parents=True)
        (sub / 'agent-a.jsonl').write_text(jl(self.reply(t0 + 1800, 'm4'), self.reply(t0 + 9000, 'm5')))
        (self.proj / 's2.jsonl').write_text(jl(self.reply(t0, 'm6', sid='s2')))   # 没有 session-owner（旧版本）：认不出是谁的
        last1 = {'input_tokens': 10000, 'cached_input_tokens': 8000, 'output_tokens': 500, 'reasoning_output_tokens': 100, 'total_tokens': 10500}
        tot1 = dict(last1)
        last2 = {'input_tokens': 2000, 'cached_input_tokens': 0, 'output_tokens': 100, 'reasoning_output_tokens': 0, 'total_tokens': 2100}
        tot2 = {'input_tokens': 12000, 'cached_input_tokens': 8000, 'output_tokens': 600, 'reasoning_output_tokens': 100, 'total_tokens': 12600}
        last3 = {'input_tokens': 1000, 'cached_input_tokens': 0, 'output_tokens': 0, 'reasoning_output_tokens': 0, 'total_tokens': 1000}
        tot3 = {'input_tokens': 13000, 'cached_input_tokens': 8000, 'output_tokens': 600, 'reasoning_output_tokens': 100, 'total_tokens': 13600}
        (self.rollouts / 'rollout-a.jsonl').write_text(jl(
            self.codex(t0, 'session_meta', {'id': 'th-1', 'creator_user_id': U3, 'model_provider': 'openai'}),
            self.codex(t0, 'event_msg', {'type': 'thread_settings_applied',
                                         'thread_settings': {'model': 'gpt-6-astra', 'service_tier': 'priority', 'model_provider_id': 'openai'}}),
            self.tokens(t0 + 10, tot1, last1),
            self.tokens(t0 + 11, tot1, last1),   # 同一次请求又报了一遍（只是额度更新了）
            self.codex(t0 + 12, 'event_msg', {'type': 'token_count', 'info': None, 'rate_limits': {}}),
            self.tokens(t0 + 20, tot2, last2),
            self.codex(t0 + 21, 'session_meta', {'id': 'th-0', 'creator_user_id': U4, 'model_provider': 'openai'}),   # 复制进来的历史
            self.codex(t0 + 22, 'turn_context', {'model': 'gpt-6-sol'}),
            self.tokens(t0 + 30, tot3, last3)))
        (self.rollouts / 'rollout-b.jsonl').write_text(jl(
            self.codex(t0, 'session_meta', {'id': 'th-2', 'creator_user_id': U3, 'model_provider': 'cca'}),   # 第三方的接口：不算
            self.codex(t0, 'turn_context', {'model': 'gpt-6-astra'}),
            self.tokens(t0 + 5, tot1, last1)))

    def test_scan_and_increments(self):
        self.write_all()
        s = self.scan()
        astra = (2000 * 10 + 8000 * 1 + 500 * 50) * 2
        self.assertEqual(self.total('claude:' + U1), [12000, 3, 3000, 300, 0, 0, 1],
                         'U1：m1、m2（没有标价）、子代理在换号以前的 m4；重复的、<synthetic>、只是提到这些字的行都不算')
        self.assertEqual(self.total('claude:' + U2), [12000, 2, 2000, 200, 0, 0, 0], 'U2：换号以后的 m3，子代理按时间认的 m5')
        self.assertEqual(self.total(None), [6000, 1, 1000, 100, 0, 0, 0], '没有 session-owner 的：记成认不出是谁的')
        self.assertEqual(self.total('codex:' + U3)[:2], [astra + (2000 * 10 + 100 * 50) * 2 + 1000 * 2 * 2, 3],
                         'Codex：重复报的不算；后面的 session_meta 是复制进来的历史，不换人；换了模型照新的算，档位不变')
        self.assertNotIn('codex:' + U4, self.j['h'])
        self.assertEqual(list(self.j['um']['claude:' + U1]), ['claude-unknown-1'])
        self.assertEqual((s.files, s.records), (5, 9))
        self.assertTrue(self.j['done'])
        # 增量：没变的不重读；写到一半的行等写完才算；从头改写的文件重读，靠去重不重复计
        s = self.scan()
        self.assertEqual((s.records, s.bytes), (0, 0))
        with open(self.proj / 's1.jsonl', 'a') as f:
            f.write(json.dumps(self.reply(self.t0 + 9600, 'm7'), separators=(',', ':')))
        self.assertEqual(self.scan().records, 0, '还没写完的一行（没有换行）不算')
        with open(self.proj / 's1.jsonl', 'a') as f:
            f.write('\n')
        self.assertEqual(self.scan().records, 1)
        self.assertEqual(self.total('claude:' + U2)[1], 3, '接着读的还是 U2 的（读到哪、是谁的都记着）')
        (self.proj / 's2.jsonl').write_text(jl({'type': 'summary', 'summary': 'x'}, self.reply(self.t0, 'm6', sid='s2'),
                                                self.reply(self.t0 + 60, 'm8', sid='s2')))
        self.assertEqual(self.scan().records, 1, '改写过（开头变了）：从头读，读过的 m6 不重复计')
        tmp = self.proj / 's2.tmp'
        tmp.write_text((self.proj / 's2.jsonl').read_text())
        os.replace(tmp, self.proj / 's2.jsonl')
        self.assertEqual(self.scan().records, 0, '换成了另一个文件（内容一样）：从头读，都去重了')
        self.assertEqual(self.total(None)[1], 2)
        with open(self.proj / 's2.jsonl', 'a') as f:
            f.write(jl(self.reply(time.time() - 40 * 86400, 'm9', sid='s2')))
        self.assertEqual(self.scan().records, 0, '第一次全读以后，35 天以前的回复不算（去重的记录已经清掉了）')
        (self.proj / 's1' / 'subagents' / 'agent-a.jsonl').unlink()
        self.scan()
        self.assertFalse([p for p in self.j['files'] if 'agent-a' in p], '删掉的文件不再记着')
        with open(self.proj / 's1.jsonl', 'a') as f:
            f.write('{"type":"assistant","usage":{broken\n')
            f.write(jl(self.reply(self.t0 + 9700, 'm10')))
        self.assertEqual(self.scan().records, 1, '坏的一行跳过，后面的照读')

    def test_run_and_publish(self):
        """cost_run：统计、认出各个身份是谁（cca 的账号目录、~/.claude.json、~/.codex/auth.json）、发布 local.json"""
        self.write_all()
        ns = self.ns
        (self.home / '.claude.json').write_text(json.dumps({'oauthAccount': {'accountUuid': U1, 'emailAddress': 'Alice@Example.com'}}))
        (self.cca / 'accounts.json').write_text(json.dumps({'bob(bob@example.com)': 'US-A', 'kimi': {'base_url': 'https://api.example.com'}}))
        cd = Path(ns['cdir']('bob(bob@example.com)'))
        cd.mkdir(parents=True)
        (cd / '.claude.json').write_text(json.dumps({'oauthAccount': {'accountUuid': U2, 'emailAddress': 'bob@example.com'}}))
        claims = base64.urlsafe_b64encode(json.dumps({'email': 'carol@example.com', 'https://api.openai.com/auth': {
            'chatgpt_user_id': U3, 'chatgpt_plan_type': 'pro'}}).encode()).decode().rstrip('=')
        (self.home / '.codex' / 'auth.json').write_text(json.dumps({'tokens': {'id_token': 'e30.%s.sig' % claims}}))
        scan, sync = ns['cost_run']()
        self.assertEqual((scan['state'], scan['first'], scan['records'], sync['state']), ('ready', True, 9, 'skipped'))
        local = json.loads((self.cca / 'cost' / 'local.json').read_text())
        self.assertEqual(local['who'], {'claude:' + U1: 'alice@example.com', 'claude:' + U2: 'bob@example.com', 'codex:' + U3: 'carol@example.com'})
        self.assertEqual(sorted(local['acc']), ['claude:' + U1, 'claude:' + U2, 'codex:' + U3])
        self.assertEqual(local['um'], {'claude:' + U1: ['claude-unknown-1']})
        self.assertEqual(sum(r[1] for r in local['orphan'].values()), 1)
        for name in ('local.json', 'scan.json'):
            self.assertEqual(oct(os.stat(self.cca / 'cost' / name).st_mode & 0o777), '0o600', name)
        self.assertFalse((self.cca / 'cost' / 'worker.json').exists(), '跑完删掉 worker.json')
        h, um = ns['cost_content'](local, ns['CLAUDE'], 'alice@example.com')
        self.assertEqual((sum(r[0] for r in h.values()), um), (12000, ['claude-unknown-1']))
        self.assertEqual(ns['cost_content'](local, ns['CODEX'], 'alice@example.com'), ({}, []), '工具分开')
        before = (self.cca / 'cost' / 'local.json').stat().st_mtime_ns
        time.sleep(0.01)
        scan, _ = ns['cost_run']()
        self.assertEqual((scan['first'], scan['records']), (False, 0))
        self.assertEqual((self.cca / 'cost' / 'local.json').stat().st_mtime_ns, before, '内容没变不重写（App 看它变了才重读）')
        (self.home / '.claude.json').write_text('{}')
        ns['cost_run']()
        self.assertEqual(json.loads((self.cca / 'cost' / 'local.json').read_text())['who'].get('claude:' + U1), 'alice@example.com',
                         '退出登录了：以前记下的身份照样认得')
        j = json.loads((self.cca / 'cost' / 'scan.json').read_text())
        j['build'] = 'old'
        (self.cca / 'cost' / 'scan.json').write_text(json.dumps(j))
        self.assertEqual(ns['cost_run']()[0]['records'], 9, '价格表、规则变了：从头重算')
        local = json.loads((self.cca / 'cost' / 'local.json').read_text())
        self.assertEqual(local['who'].get('claude:' + U1), 'alice@example.com', '从头重算：退出登录了的 alice 照样认得（以前记下的留着）')
        self.assertEqual(sum(r[0] for r in ns['cost_content'](local, ns['CLAUDE'], 'alice@example.com')[0].values()), 12000)
        (self.cca / 'cost' / 'scan.json').unlink()
        self.assertEqual(ns['cost_run']()[0]['records'], 9, 'scan.json 没了（restore 的备份不带它）：从头重算')
        self.assertEqual(json.loads((self.cca / 'cost' / 'local.json').read_text())['who'].get('claude:' + U1), 'alice@example.com',
                         '身份照 local.json 里记下的')
        (self.cca / 'config.json').unlink()
        with self.assertRaises(OSError):
            ns['cost_run']()   # cca restore 刚删掉 ~/.cca：不写回来
        self.assertFalse((self.cca / 'config.json').exists())

    def test_rebuild_keeps_history(self):
        """从头重算（价格表、规则变了）：早于现在还在的最早的会话记录（修改时间）的小时，会话记录可能删掉了一部分（Claude Code
        默认删掉 30 天以前的），以前算过的沿用以前的；以后的照会话记录重算。scan.json 没了（restore）就用 local.json 里的"""
        self.write_all()
        ns = self.ns
        (self.home / '.claude.json').write_text(json.dumps({'oauthAccount': {'accountUuid': U1, 'emailAddress': 'alice@example.com'}}))
        ns['cost_run']()
        lo = self.t0 - 86400   # 会话记录的修改时间都改成 t0 的前一天：从这时起会话记录是全的
        for p in self.home.rglob('*.jsonl'):
            os.utime(p, (lo, lo))
        scan = self.cca / 'cost' / 'scan.json'
        j = json.loads(scan.read_text())
        a, ident = j['h']['claude:' + U1], 'claude:' + U1
        gone, late, real = str(lo - 10 * 86400), str(self.t0 + 5 * 3600), str(self.t0)
        want = a[real]
        a[gone] = [5000000, 1, 10, 1, 0, 0, 0]   # 早于它：那个会话记录删掉了，重算算不出来
        a[late] = [7000000, 1, 10, 1, 0, 0, 0]   # 晚于它、会话记录里没有：以前算错了（规则改了）
        a[real] = [99, 1, 0, 0, 0, 0, 0]         # 晚于它：照会话记录重算
        j['orphan'][gone] = [1000, 1, 0, 0, 0, 0, 0]
        j['build'] = 'old'
        scan.write_text(json.dumps(j))
        self.assertEqual(ns['cost_run']()[0]['records'], 9)
        j = json.loads(scan.read_text())
        self.assertNotIn('old', j, '挑完就不留了')
        self.assertEqual(j['h'][ident].get(gone), [5000000, 1, 10, 1, 0, 0, 0], '会话记录可能删掉了的小时：沿用以前算好的')
        self.assertNotIn(late, j['h'][ident], '会话记录是全的小时：照重算的')
        self.assertEqual(j['h'][ident][real], want)
        self.assertEqual(j['orphan'].get(gone), [1000, 1, 0, 0, 0, 0, 0], '认不出是谁的同样')
        local = json.loads((self.cca / 'cost' / 'local.json').read_text())
        self.assertEqual(local['acc'][ident].get(gone), [5000000, 1, 10, 1, 0, 0, 0])
        scan.unlink()   # restore 的备份不带 scan.json：从 local.json 里取
        ns['cost_run']()
        self.assertEqual(json.loads(scan.read_text())['h'][ident].get(gone), [5000000, 1, 10, 1, 0, 0, 0], '从 local.json 里取')
        j = json.loads(scan.read_text())
        j['build'], j['old'] = 'old', {'h': {ident: {gone: [8000000, 2, 0, 0, 0, 0, 0]}}, 'orphan': {}}   # 上一次重算没算完
        scan.write_text(json.dumps(j))
        ns['cost_run']()
        self.assertEqual(json.loads(scan.read_text())['h'][ident].get(gone), [8000000, 2, 0, 0, 0, 0, 0],
                         '上一次重算没算完又要从头来：用那一次留着的（重算到一半的不算以前的）')
        for p in self.home.rglob('*.jsonl'):
            p.unlink()
        j = json.loads(scan.read_text())
        j['build'] = 'old'
        scan.write_text(json.dumps(j))
        ns['cost_run']()
        self.assertEqual(json.loads(scan.read_text())['h'][ident].get(real), want, '会话记录一个都没了：以前算的全留着')

    def test_sync_command(self):
        ns = self.ns
        out = []
        with mock.patch('builtins.print', lambda *a, **k: out.append(' '.join(map(str, a)))), self.assertRaises(SystemExit) as e:
            ns['cmd_cost_sync']('--json')
        self.assertEqual(e.exception.code, 2, '没有官方账号：用不了')
        (self.cca / 'accounts.json').write_text(json.dumps({'bob(bob@example.com)': 'US-A'}))
        (self.cca / 'config.json').write_text('{"cost": false}')
        with mock.patch('builtins.print', lambda *a, **k: out.append(' '.join(map(str, a)))), self.assertRaises(SystemExit) as e:
            ns['cmd_cost_sync']('--json')
        self.assertEqual(e.exception.code, 2)
        self.assertIn('"cost": false', out[-1])
        (self.cca / 'config.json').write_text('{}')
        self.write_all()
        out.clear()
        with mock.patch('builtins.print', lambda *a, **k: out.append(' '.join(map(str, a)))):
            ns['cmd_cost_sync']('--json', '--now')
        doc = json.loads(out[-1])
        self.assertEqual((doc['version'], doc['kind'], doc['scan']['state'], doc['scan']['records'], doc['sync']['state']),
                         (1, 'cost_sync', 'ready', 9, 'skipped'), '没开设备统计：不同步')
        lock = open(self.cca / 'cost' / 'cost.lock', 'a')
        import fcntl
        fcntl.flock(lock, fcntl.LOCK_EX)
        try:
            with mock.patch.dict(ns, {'COST_WAIT': 0.2}), self.assertRaises(SystemExit) as e, \
                    mock.patch('builtins.print', lambda *a, **k: out.append(' '.join(map(str, a)))):
                ns['cmd_cost_sync']('--json')
            self.assertEqual(e.exception.code, 1, '另一个统计一直没结束')
        finally:
            lock.close()


class Periods(Base):
    TZ = 'America/New_York'

    def test_days_across_dst(self):
        now = time.mktime((2026, 11, 13, 12, 0, 0, 0, 0, -1))
        days = self.ns['cost_days'](now)
        self.assertEqual(len(days), 30)
        self.assertTrue(days[-1][0] <= now < days[-1][1], '最后一项是今天')
        self.assertTrue(all(a[1] == b[0] for a, b in zip(days, days[1:])), '首尾相接')
        self.assertEqual(days[0][2], '2026-10-15')
        self.assertEqual(sorted(e - s for s, e, _ in days).count(25 * 3600), 1, '夏令时结束的那天（11 月 1 日）25 个小时')
        self.assertEqual([d for s, e, d in days if e - s != 86400], ['2026-11-01'])
        self.assertTrue(all(time.localtime(s)[3:6] == (0, 0, 0) for s, _, _ in days), '本地时间零点')

    def test_calendar_weeks(self):
        now = time.mktime((2026, 11, 13, 12, 0, 0, 0, 0, -1))
        weeks = self.ns['cost_weeks'](None, now)
        self.assertEqual(len(weeks), 13, '最近 12 周和这一周')
        self.assertTrue(all(a[1] == b[0] for a, b in zip(weeks, weeks[1:])))
        self.assertTrue(all(b == 'calendar' and time.localtime(s).tm_wday == 0 and time.localtime(s)[3:6] == (0, 0, 0) for s, e, b in weeks),
                        '周一零点（本地时间）开始')
        self.assertTrue(weeks[-1][0] <= now < weeks[-1][1])
        self.assertEqual(time.strftime('%Y-%m-%d', time.localtime(weeks[-1][1])), '2026-11-16', '这一周到下周一零点')
        dst = [(s, e) for s, e, _ in weeks if e - s != 7 * 86400]
        self.assertEqual([e - s for s, e in dst], [7 * 86400 + 3600], '跨夏令时结束的那一周多一个小时')

    def test_quota_weeks(self):
        """有额度窗口的地方用它，前后空着的按日历周补；不到一天的零头并进前一项"""
        now = time.mktime((2026, 11, 13, 12, 0, 0, 0, 0, -1))
        r1 = time.mktime((2026, 10, 28, 10, 30, 0, 0, 0, -1))   # 周三 10:30 重置
        quota = [{'start_epoch': r1 - 7 * 86400, 'end_epoch': r1}, {'start_epoch': r1, 'end_epoch': r1 + 7 * 86400},
                 {'start_epoch': r1 + 7 * 86400 + 3 * 86400, 'end_epoch': r1 + 17 * 86400}]   # 中间断了 3 天；最后一项包含 now
        weeks = self.ns['cost_weeks'](quota, now)
        self.assertTrue(all(a[1] == b[0] for a, b in zip(weeks, weeks[1:])), weeks)
        self.assertTrue(weeks[-1][0] <= now < weeks[-1][1])
        self.assertEqual([b for _, _, b in weeks][-5:], ['calendar', 'quota', 'quota', 'calendar', 'quota'])
        gap = weeks[-2]
        self.assertEqual((gap[0], gap[1]), (r1 + 7 * 86400, r1 + 10 * 86400), '断开的 3 天：日历周补上（不到下个周一就截住）')
        self.assertEqual(weeks[-1][1], r1 + 17 * 86400, '这一周到额度窗口结束')
        before = weeks[-5]
        self.assertEqual(before[1], r1 - 7 * 86400)
        self.assertEqual(time.strftime('%a %H:%M', time.localtime(before[0])), 'Mon 00:00',
                         '额度窗口前面一周的零头（周一到周三 10:30）比一天长：单独一项')
        s5 = time.mktime((2026, 10, 19, 5, 0, 0, 0, 0, -1))   # 周一 05:00 开始的额度窗口：前面只有 5 个小时
        w2 = self.ns['cost_weeks']([{'start_epoch': s5, 'end_epoch': s5 + 7 * 86400}], now)
        idx = [i for i, p in enumerate(w2) if p[2] == 'quota'][0]
        self.assertEqual((w2[idx - 1][0], w2[idx - 1][1]), (time.mktime((2026, 10, 12, 0, 0, 0, 0, 0, -1)), s5), '不到一天的零头并进前一项')
        self.assertTrue(all(a[1] == b[0] for a, b in zip(w2, w2[1:])) and w2[-1][0] <= now < w2[-1][1])
        self.assertTrue(len(w2) <= 13 and len(weeks) <= 13)

    def test_bin_by_midpoint(self):
        b = self.ns['cost_bin']
        periods = [(1000 * 3600, 1000 * 3600 + 5400, 'x'), (1000 * 3600 + 5400, 1003 * 3600, 'y')]
        h = {str(1000 * 3600): [1, 1, 0, 0, 0, 0, 0], str(1001 * 3600): [2, 1, 0, 0, 0, 0, 0], str(1002 * 3600): [4, 1, 0, 0, 0, 0, 0],
             str(999 * 3600): [8, 1, 0, 0, 0, 0, 0], str(1003 * 3600): [16, 1, 0, 0, 0, 0, 0]}
        self.assertEqual([r[0] for r in b(periods, h)], [1, 6], '跨过边界的小时按中点算；时段以外的不算')


class View(Base):
    TZ = 'UTC'
    NOW = 1794571200.0   # 2026-11-13（周五）12:00 UTC

    def at(self, y, mo, d, hh):
        return str(int(time.mktime((y, mo, d, hh, 0, 0, 0, 0, -1))))

    def row(self, usd, req=1, unpriced=0):
        return [int(usd * M), req, 1000 * req, 100 * req, 0, 0, unpriced]

    def setUp(self):
        super().setUp()
        cost = self.cca / 'cost'
        cost.mkdir()
        self.local = {'v': 1, 'build': 'x', 'who': {'claude:' + U1: 'alice@example.com', 'codex:' + U3: 'alice@example.com'},
                      'acc': {'claude:' + U1: {self.at(2026, 10, 1, 10): self.row(1), self.at(2026, 11, 3, 10): self.row(7, 3, 1),
                                               self.at(2026, 11, 10, 10): self.row(3), self.at(2026, 11, 13, 9): self.row(5, 2)},
                              'codex:' + U3: {self.at(2026, 11, 12, 8): self.row(4)}},
                      'um': {'claude:' + U1: ['claude-unknown-1']},
                      'orphan': {self.at(2026, 11, 12, 1): self.row(0.5), self.at(2026, 9, 1, 1): self.row(100)}}
        now = self.NOW
        slot = {'SHA256:bob': {'h': {self.at(2026, 11, 12, 10): self.row(2)}, 'um': ['claude-other'], 'at': now - 600, 'name': 'bob-mini',
                               'revoked': False, 'tag': 't1'},
                'SHA256:bob-old': {'h': {self.at(2026, 10, 20, 10): self.row(1), self.at(2026, 11, 12, 11): self.row(9)}, 'um': [],
                                   'at': now - 30 * 86400, 'name': 'bob-mini', 'revoked': True, 'tag': 't2'},
                'SHA256:carol': {'h': {self.at(2026, 11, 11, 10): self.row(1)}, 'um': [], 'at': now - 2 * 86400, 'name': 'carol-air',
                                 'revoked': False, 'tag': 't3'},
                'SHA256:junk': {'h': {'x': [1]}, 'at': now, 'name': 'junk', 'tag': 't4'}}
        self.remote = {'v': 1, 'acc': {'claude:alice@example.com': slot}}
        self.sync = {'v': 1, 'state': 'ok', 'at': now - 100, 'ok_at': now - 100, 'me': 'alice-mbp',
                     'acc': {'claude:alice@example.com': {'checked': now - 100, 'missing': [
                         {'name': 'dan-air', 'used_epoch': now - 3600}, {'name': 'old-air', 'used_epoch': now - 60 * 86400}]}}}
        self.cfg = {'url': 'https://192.0.2.10:18443', 'pins': [], 'salt': '00' * 16, 'key': '/x', 'fp': 'SHA256:me'}
        self.write()
        (cost / 'scanned').write_text('')

    def write(self):
        for name, doc in (('local.json', self.local), ('remote.json', self.remote), ('sync.json', self.sync)):
            (self.cca / 'cost' / name).write_text(json.dumps(doc))

    def view(self, c=None, cfg='default'):
        return self.ns['CostView'](c or {}, self.NOW, self.cfg if cfg == 'default' else cfg)

    def test_account(self):
        ns = self.ns
        cv = self.view()
        a = cv.account(ns['CLAUDE'], 'alice(alice@example.com)', None, True, True)
        self.assertEqual((a['currency'], a['prices_date'], len(a['weeks']), len(a['days'])), ('USD', ns['COST_PRICES_DATE'], 13, 30))
        s = a['summary']
        self.assertEqual((s['weeks_usd'], s['current_usd'], s['days_usd'], s['today_usd']), (20.0, 11.0, 19.0, 5.0),
                         '三台设备加起来；撤销的那份只算 bob-mini 现在的 key 有数据以前的小时；坏的不算')
        self.assertEqual((s['average_usd'], s['average_weeks']), (1.6, 5), '平均：从有数据的那一周以后、过完的整周')
        self.assertEqual((s['average_day_usd'], s['average_days']), (0.4828, 29),
                         '按天的平均：从有数据的那天以后、过完的整天（10 月 15 日到 11 月 12 日，今天不算）：14 美元 / 29 天')
        self.assertEqual((s['requests'], s['unpriced_requests']), (10, 1))
        self.assertEqual(a['since_epoch'], float(self.at(2026, 10, 1, 10)))
        self.assertEqual([w['partial'] for w in a['weeks']][:8], [True] * 7 + [False], '有数据以前的周：数据不全')
        self.assertTrue(a['weeks'][-1]['current'] and not any(w['current'] for w in a['weeks'][:-1]))
        self.assertEqual(a['days'][-1]['date'], '2026-11-13')
        self.assertEqual(a['days'][-1]['tokens'], {'input': 2000, 'output': 200, 'cache_read': 0, 'cache_write': 0})
        self.assertEqual(a['unpriced_models'], ['claude-other', 'claude-unknown-1'])
        devs = a['devices']
        self.assertEqual([(d['name'], d['me'], d['revoked'], d['stale']) for d in devs],
                         [('alice-mbp', True, False, False), ('bob-mini', False, False, False), ('carol-air', False, False, True)],
                         '这台设备在最前，别的按最近 30 天的多少排；两天没同步的标出来')
        self.assertEqual(devs[0]['id'], 'SHA256:me')
        self.assertEqual({k: devs[0]['summary'][k] for k in ('average_usd', 'average_weeks', 'average_day_usd', 'average_days')},
                         {'average_usd': 1.4, 'average_weeks': 5, 'average_day_usd': 0.3448, 'average_days': 29},
                         '一台设备的平均按整个账号的那几周、那几天算（各台设备的平均加起来是整个账号的）')
        self.assertEqual(devs[1]['summary']['weeks_usd'], 3.0, 'bob-mini：现在的 2 + 撤销以前的 1（撤销以后的 9 不重复计）')
        self.assertEqual(sum(devs[0]['days_usd']), 15.0)
        self.assertEqual(len(devs[0]['weeks_usd']), 13)
        self.assertEqual(a['sync'], {'state': 'ok', 'message': None, 'checked_epoch': self.NOW - 100,
                                     'missing': [{'name': 'dan-air', 'used_epoch': self.NOW - 3600}]}, '30 天以前用过的不提')
        x = cv.account(ns['CODEX'], 'alice@example.com', None, True, True)
        self.assertEqual((x['summary']['weeks_usd'], [d['name'] for d in x['devices']]), (4.0, ['alice-mbp']), '工具分开')
        self.assertIsNone(cv.account(ns['CLAUDE'], 'bob@example.com', None, True, True), '没有记录：None')
        t = cv.total(['claude', 'codex'])
        self.assertEqual((t['current_usd'], t['weeks_usd'], t['average_usd'], t['days_usd'], t['today_usd']), (15.0, 24.0, 1.6, 23.0, 5.0))
        self.assertEqual(t['average_day_usd'], 0.4828, '按天的平均：有平均的账号加起来（Codex 只有一天、还没过完的整天）')
        self.assertEqual(t['by_tool']['codex'], {'current_usd': 4.0, 'weeks_usd': 4.0, 'average_usd': None, 'average_day_usd': None,
                                                 'days_usd': 4.0, 'today_usd': 0.0})
        self.assertEqual(t['by_tool']['claude']['weeks_usd'], 20.0)
        self.assertEqual((len(t['days']), t['days'][-2]['usd'], t['days'][-1]['current']), (30, 6.0, True), '11 月 12 日：bob-mini 的 2 + Codex 的 4')
        me = t['me']
        self.assertEqual({k: me[k] for k in ('current_usd', 'weeks_usd', 'average_usd', 'average_day_usd', 'days_usd', 'today_usd')},
                         {'current_usd': 12.0, 'weeks_usd': 20.0, 'average_usd': 1.4, 'average_day_usd': 0.3448, 'days_usd': 19.0, 'today_usd': 5.0},
                         'me：各账号里这台设备的那一份加起来（Claude 的 alice-mbp，Codex 只有这台）')
        self.assertEqual(me['by_tool']['claude'], {'current_usd': 8.0, 'weeks_usd': 16.0, 'average_usd': 1.4, 'average_day_usd': 0.3448,
                                                   'days_usd': 15.0, 'today_usd': 5.0})
        self.assertEqual((len(me['days']), me['days'][-2]['usd'], me['days'][-1]['usd'], me['days'][-1]['date']), (30, 4.0, 5.0, '2026-11-13'),
                         '11 月 12 日只有 Codex 的 4（bob-mini 的不算）')
        self.assertEqual([d['start_epoch'] for d in me['days']], [d['start_epoch'] for d in t['days']])
        self.assertEqual(t['unattributed'], {'days_usd': 0.5, 'requests': 1}, '认不出是谁的：最近 30 天的')
        self.assertEqual(t['scan']['state'], 'ready')
        quota = {'weeks': [{'start_epoch': self.NOW - 2 * 86400, 'end_epoch': self.NOW + 5 * 86400}]}
        q = self.view().account(ns['CLAUDE'], 'alice@example.com', quota, True, True)
        self.assertEqual((q['weeks'][-1]['basis'], q['weeks'][-1]['start_epoch'], q['summary']['current_usd']),
                         ('quota', self.NOW - 2 * 86400, 7.0), '有额度窗口时，这一周按额度窗口算（周三 12:00 起：carol-air 周三 10 点的不算）')

    def test_me_without_records(self):
        ns = self.ns
        self.local['acc']['claude:' + U1] = {}
        self.write()
        cv = self.view()
        a = cv.account(ns['CLAUDE'], 'alice(alice@example.com)', None, True, True)
        self.assertEqual([(d['name'], d['me']) for d in a['devices']], [('bob-mini', False), ('carol-air', False)],
                         '这台设备没有这个账号的记录：不列这台设备，别的设备的照样有')
        cv.account(ns['CODEX'], 'alice@example.com', None, True, True)
        t = cv.total(['claude', 'codex'])
        self.assertEqual((t['days_usd'], t['me']['days_usd'], t['me']['by_tool']['claude']['days_usd'], t['me']['by_tool']['claude']['average_usd']),
                         (8.0, 4.0, 0.0, None), 'me 里没有这个账号（金额是 0、没有平均），只有 Codex 的 4')

    def test_sync_states(self):
        ns, C, k = self.ns, self.ns['CLAUDE'], 'alice(alice@example.com)'
        st = lambda cv, mapped=True, logged=True: cv.account(C, k, None, mapped, logged)['sync']
        self.assertEqual(st(self.view({'presence': False}))['state'], 'off')
        lo = self.view(cfg=None)
        a = lo.account(C, k, None, True, True)
        self.assertEqual((a['sync']['state'], [d['name'] for d in a['devices']], a['devices'][0]['id']), ('local_only', [os.uname().nodename.split('.')[0]], 'local'),
                         '没有设备统计：只有这台设备的')
        self.assertEqual(st(self.view(), mapped=False)['state'], 'local_only', '不在映射里的账号')
        self.assertEqual(st(self.view(), logged=False)['state'], 'not_logged_in')
        self.sync['state'] = 'old_server'
        self.write()
        s = st(self.view())
        self.assertEqual((s['state'], s['message']), ('old_server', ns['COST_OLD']))
        self.sync.update(state='error', error='连不上', at=self.NOW - 50)
        self.write()
        self.assertEqual((st(self.view())['state'], st(self.view())['message']), ('error', '连不上'), '这个账号上次同步好了以后又出错了')
        self.sync['at'] = self.NOW - 200
        self.write()
        self.assertEqual(st(self.view())['state'], 'ok', '出错是这个账号同步好以前的事')
        del self.sync['acc']['claude:alice@example.com']['checked']
        self.sync['state'] = 'ok'
        self.write()
        self.assertEqual(st(self.view())['state'], 'pending', '还没同步过')
        self.assertIsNone(self.view({'cost': False}).total(['claude']), '关掉了')
        self.assertIsNone(self.view().total(['claude']), '没有官方账号（account 一次都没调）')

    def test_scan_state(self):
        (self.cca / 'cost' / 'scanned').unlink()
        self.assertEqual(self.view().scan_state(), 'never')
        me = os.getpid()
        (self.cca / 'cost' / 'worker.json').write_text(json.dumps({'v': 1, 'pid': me, 'started': self.ns['proc_starts']({me})[me]}))
        self.assertEqual(self.view().scan_state(), 'scanning')
        (self.cca / 'cost' / 'worker.json').write_text(json.dumps({'v': 1, 'pid': me, 'started': 1}))
        self.assertEqual(self.view().scan_state(), 'never', '进程号被重用了')


class Crypto(Base):
    def test_seal_open(self):
        ns, C, X = self.ns, self.ns['CLAUDE'], self.ns['CODEX']
        salt, aid, fp = 'ab' * 16, 'a' * 32, 'SHA256:' + 'F' * 43
        k = ns['keyshare_keys'](salt, C, 'Alice@Example.com', U1, ns['COST_LABEL'])
        self.assertEqual(k, sw.derive(sw.COST, salt, 'claude', 'alice@example.com', U1), '和文档写的一样')
        self.assertNotEqual(k, ns['keyshare_keys'](salt, C, 'alice@example.com', U1), '和共享 API key 的密钥不通')
        now = time.time()
        doc = {'v': 1, 'at': now, 'h': {'1794567600': [1, 2, 3, 4, 5, 6, 0]}, 'um': ['claude-x']}
        b = ns['cost_seal'](k, aid, fp, doc)
        self.assertEqual(sw.cost_open(k, aid, fp, b), doc, '照文档能解开')
        self.assertEqual(ns['cost_open'](k, aid, fp, b, now), {'at': now, 'h': doc['h'], 'um': ['claude-x']})
        self.assertEqual(ns['cost_open'](k, aid, fp, sw.cost_seal(k, aid, fp, doc), now)['h'], doc['h'], '照文档封的 cca 也认')
        self.assertEqual(srvmod.parse_blob(b, srvmod.MAX_COST_BLOB), b, '服务器收这个形状')
        self.assertEqual(len(base64.b64decode(b['c'])) % 1024, 0)
        self.assertNotEqual(ns['cost_seal'](k, aid, fp, doc)['c'], b['c'], '每次的 nonce 不同')
        for o, a2, f2 in ((ns['keyshare_keys'](salt, C, 'alice@example.com', U2, ns['COST_LABEL']), aid, fp),   # 身份不对
                          (ns['keyshare_keys'](salt, X, 'alice@example.com', U1, ns['COST_LABEL']), aid, fp),   # 同一个邮箱的 Codex 账号
                          (k, 'b' * 32, fp), (k, aid, 'SHA256:' + 'G' * 43)):   # 挪到别的账号、别的设备上
            self.assertIsNone(ns['cost_open'](o, a2, f2, b, now))
        c = bytearray(base64.b64decode(b['c']))
        c[7] ^= 1
        self.assertIsNone(ns['cost_open'](k, aid, fp, dict(b, c=base64.b64encode(bytes(c)).decode()), now), '改过的')
        for junk in (None, {}, 'x', {'n': 'x', 'c': 'y', 't': 'z'}, dict(b, n=12), dict(b, n=b['t'])):
            self.assertIsNone(ns['cost_open'](k, aid, fp, junk, now), junk)
        bad_docs = ({'v': 2, 'at': now, 'h': {}, 'um': []}, {'v': 1, 'at': now + 3 * 86400, 'h': {}, 'um': []},
                    {'v': 1, 'at': 'x', 'h': {}, 'um': []}, {'v': 1, 'at': now, 'h': {'x': [1] * 7}, 'um': []},
                    {'v': 1, 'at': now, 'h': {'1794567600': [1] * 6}, 'um': []}, {'v': 1, 'at': now, 'h': {'1794567600': [-1] * 7}, 'um': []},
                    {'v': 1, 'at': now, 'h': {}, 'um': 'x'}, [1])
        for d in bad_docs:
            self.assertIsNone(ns['cost_open'](k, aid, fp, sw.cost_seal(k, aid, fp, d), now), '认证过、内容不对的也不认：%s' % (d,))
        many = {str(1794567600 - 3600 * i): [0] * 7 for i in range(24 * 7 * 20 + 1)}
        self.assertIsNone(ns['cost_open'](k, aid, fp, sw.cost_seal(k, aid, fp, {'v': 1, 'at': now, 'h': many, 'um': []}), now), '太多小时')
        um = ['ok-model', 5, 'x' * 65, 'bad\nname'] + ['m%d' % i for i in range(40)]
        self.assertEqual(ns['cost_open'](k, aid, fp, sw.cost_seal(k, aid, fp, {'v': 1, 'at': now, 'h': {}, 'um': um}), now)['um'],
                         ['ok-model'] + ['m%d' % i for i in range(31)], '没有标价的模型名：只收能显示的、最多 32 个')
        bomb = sw.cost_seal(k, aid, fp, None, raw=zlib.compress(b'{"v":1,"um":["' + b'x' * (5 << 20) + b'"]}'))
        self.assertIsNone(ns['cost_open'](k, aid, fp, bomb, now), '解压出来太大的不要')
        lie = sw.seal(sw.COST, k, [aid, fp], (5000).to_bytes(4, 'big') + bytes(1020))
        self.assertIsNone(ns['cost_open'](k, aid, fp, lie, now), '长度比内容长')
        h = {str(1794567600 - 3600 * i): [123456789, 50, 5000000, 300000, 90000000, 2000000, 1] for i in range(14 * 7 * 24)}
        big = ns['cost_seal'](k, aid, fp, {'v': 1, 'at': now, 'h': h, 'um': []})
        self.assertLessEqual(len(big['c']), ns['COST_BLOB_MAX'], '14 周每个小时都有记录也放得下')
        self.assertEqual(ns['COST_BLOB_MAX'], srvmod.MAX_COST_BLOB, '客户端和服务器的上限一样')
        rnd = random.Random(7)   # 压缩不了的数字，小时又多：放不下，去掉最早的直到放得下
        huge = {'h': {str(1794567600 - 3600 * i): [rnd.randrange(10 ** 12) for _ in range(7)] for i in range(14 * 7 * 24)}, 'um': []}
        self.assertGreater(len(ns['cost_seal'](k, aid, fp, dict(huge, v=1, at=now))['c']), ns['COST_BLOB_MAX'], '这份原样封放不下')
        fit = ns['cost_seal_fit'](k, aid, fp, huge, now)
        kept = ns['cost_open'](k, aid, fp, fit, now)['h']
        newest = sorted(huge['h'], key=int)[-len(kept):]
        self.assertLessEqual(len(fit['c']), ns['COST_BLOB_MAX'], '去掉一部分以后放得下')
        self.assertTrue(0 < len(kept) < len(huge['h']) and sorted(kept, key=int) == newest, '留下的是最近的那些小时')
        self.assertEqual(len(huge['h']), 14 * 7 * 24, '传进来的那份不改')


if __name__ == '__main__':
    unittest.main()
