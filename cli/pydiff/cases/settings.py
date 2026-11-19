"""共享设置的检查、说明和修复（cli/src/cmd/settings.rs）：settings-check、launch-check、settings-explain、settings-fix、
codex-fallback-off。tests/isolation_settings.py 里的场景也都在这里（按子命令跑，两边对拍）。

夹具里只用假数据：example.com / example.invalid 的地址、dummy 的 key。检查的输出只该有来源和键名，值一个字都不该出现。

这一份里的 case 比 run.py 的多一样：夹具函数可以返回 {'cwd': 相对沙箱的目录, 'argv': 参数, 'home': 相对沙箱的目录}，换掉用例的
工作目录、参数和 HOME——沙箱在哪、settings-fix 的摘要是多少（里面有文件的绝对路径），要到放夹具时才知道。"""
import datetime
import hashlib
import json
import os
import time

from run import CASES
from run import case as plain_case

TOOLS = ('claude', 'codex', 'agy')
# 备份文件名里的本地时间（文件名本身 run.py 不换：两边要落在同一秒里，见 same_second）
STAMP = [(r'\.cca-backup-\d{8}-\d{6}', '.cca-backup-STAMP')]


def case(name, argv, **kw):
    def reg(setup):
        def placed(b):
            patch = setup(b) or {}
            if 'cwd' in patch:
                entry['cwd'] = b.path(patch['cwd'])
                os.makedirs(entry['cwd'], exist_ok=True)
            if 'argv' in patch:
                entry['argv'] = patch['argv']
            if 'home' in patch:
                entry['env'] = dict(entry['env'], HOME=b.path(patch['home']))
        plain_case(name, argv, **kw)(placed)
        entry = CASES[-1]
        return setup
    return reg


def settings(b, data, name='settings.json', mode=0o600, **kw):
    """~/.claude 里的一份设置"""
    kw.setdefault('indent', None)
    return b.json('.claude/' + name, data, mode, **kw)


def toml(b, text, name='config.toml'):
    """~/.codex 里的一份配置"""
    return b.write('.codex/' + name, text)


def agy(b, data):
    return b.json('.gemini/antigravity-cli/settings.json', data)


def link(b, rel, target):
    p = b.path(rel)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    os.symlink(target, p)
    return p


def mkdir(b, rel):
    os.makedirs(b.path(rel), exist_ok=True)
    return b.path(rel)


# ---- settings-check：Claude 的共享 settings.json / settings.local.json ------------------------------------------------

@case('settings-check/claude-none', ['settings-check'])
def _(b):
    pass


@case('settings-check/claude-clean', ['settings-check'])
def _(b):
    settings(b, {'model': 'test-model', 'env': {'EDITOR': 'vi', 'API_TIMEOUT_MS': '600000', 'ANTHROPIC_MODEL': 'x'}, 'permissions': {'allow': ['Read']}})
    settings(b, {}, 'settings.local.json')


@case('settings-check/extra-argument', ['settings-check', 'x'])
def _(b):
    pass


# 每一类会盖掉账号的变量；不在名单里的（CLAUDE_CODE_USE_SOMETHING_ELSE、API_TIMEOUT_MS……）不算
CRED_NAMES = ['HTTPS_PROXY', 'HTTP_PROXY', 'http_proxy', 'ALL_PROXY', 'all_proxy', 'NO_PROXY', 'no_Proxy', 'ANTHROPIC_BASE_URL', 'ANTHROPIC_AUTH_TOKEN',
              'ANTHROPIC_API_KEY', 'anthropic_api_key', 'CLAUDE_CONFIG_DIR', 'CLAUDE_SECURESTORAGE_CONFIG_DIR', 'CLAUDE_CODE_OAUTH_TOKEN',
              'CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR', 'CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR', 'CLAUDE_CODE_GATEWAY_TOKEN_FILE_DESCRIPTOR',
              'CLAUDE_CODE_WEBSOCKET_AUTH_FILE_DESCRIPTOR', 'CLAUDE_CODE__FILE_DESCRIPTOR', 'CLAUDE_CODE_USE_BEDROCK', 'CLAUDE_CODE_USE_VERTEX',
              'CLAUDE_CODE_USE_FOUNDRY', 'CLAUDE_CODE_USE_GATEWAY', 'CLAUDE_CODE_USE_MANTLE', 'CLAUDE_CODE_USE_ANTHROPIC_AWS',
              'CLAUDE_CODE_USE_ANTHROPIC_GOOGLE_CLOUD', 'CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST']
BENIGN_NAMES = ['EDITOR', 'API_TIMEOUT_MS', 'ANTHROPIC_MODEL', 'ANTHROPIC_CUSTOM_HEADERS', 'CLAUDE_CODE_USE_SOMETHING_ELSE', 'CLAUDE_CODE_USE_', 'XHTTPS_PROXY',
                'HTTPS_PROXY_X', 'HTTPSS_PROXY', 'CLAUDE_CODE_FILE_DESCRIPTOR', 'CLAUDE_CODE_OAUTH_TOKEN2', 'ANTHROPIC_BASE_URL ', ' NO_PROXY', 'PROXY',
                'CLAUDE_CODE_1_FILE_DESCRIPTOR', 'claude_code_provider_managed_by_host_x']


@case('settings-check/claude-every-kind', ['settings-check'])
def _(b):
    env = {}
    for i, n in enumerate(CRED_NAMES + BENIGN_NAMES):
        env[n] = 'dummy-do-not-echo-%d' % i
    settings(b, {'env': env, 'apiKeyHelper': 'echo dummy-do-not-echo'})


@case('settings-check/claude-every-kind-local', ['settings-check'])
def _(b):
    # 同样的名字写在 settings.local.json 里，倒着写：先后跟着文件里的走
    settings(b, {'env': {'EDITOR': 'vi'}})
    settings(b, {'env': {n: 'https://example.invalid/dummy' for n in reversed(BENIGN_NAMES + CRED_NAMES)}}, 'settings.local.json')


for name, value in [('str', 'test-helper'), ('empty', ''), ('null', None), ('false', False), ('zero', 0), ('true', True), ('list', ['x']), ('empty-list', []),
                    ('dict', {'a': 1}), ('empty-dict', {}), ('float-zero', 0.0), ('float', 0.5)]:
    @case('settings-check/claude-apikeyhelper-%s' % name, ['settings-check'])
    def _(b, value=value):
        settings(b, {'apiKeyHelper': value, 'env': {}})


@case('settings-check/claude-both-files', ['settings-check'])
def _(b):
    settings(b, {'env': {'NO_PROXY': '*', 'HTTPS_PROXY': 'http://example.invalid'}, 'apiKeyHelper': 'x'})
    settings(b, {'env': {'HTTPS_PROXY': 'http://example.invalid', 'ANTHROPIC_API_KEY': 'dummy'}, 'apiKeyHelper': 'y'}, 'settings.local.json')


for name, text in [('list', '[]'), ('string', '"x"'), ('number', '1'), ('null', 'null'), ('true', 'true'), ('env-list', '{"env": []}'),
                   ('env-null', '{"env": null}'), ('env-string', '{"env": "HTTPS_PROXY"}'), ('env-number', '{"env": 0}'), ('env-false', '{"env": false}'),
                   ('bad-json', '{bad-json'), ('empty', ''), ('blank', ' \n'), ('truncated', '{"env": {'), ('trailing-comma', '{"env": {},}'),
                   ('bom', '﻿{"env": {"HTTPS_PROXY": "x"}}'), ('two-values', '{} {}'), ('single-quotes', "{'env': {}}"),
                   ('nan-values', '{"a": NaN, "b": [Infinity, -Infinity], "env": {"NO_PROXY": "x"}}'),
                   ('duplicate-env-last-clean', '{"env": {"HTTPS_PROXY": "x"}, "env": {}}'),
                   ('duplicate-env-last-bad', '{"env": {}, "env": {"HTTPS_PROXY": "x"}}'),
                   ('duplicate-key', '{"env": {"HTTPS_PROXY": "x", "EDITOR": "vi", "HTTPS_PROXY": "y"}}'),
                   ('crlf', '{\r\n  "env": {\r\n    "ALL_PROXY": "x"\r\n  }\r\n}\r\n'), ('cr-only', '{\r"env": {"ALL_PROXY": "x"}\r}'),
                   ('escaped-key', '{"env": {"HTTPS\\u005fPROXY": "x", "\\u0041LL_PROXY": "y"}}'),
                   ('key-with-newline', '{"env": {"HTTPS_PROXY\\n": "x", "NO_PROXY\\n\\n": "y", "\\nALL_PROXY": "z"}}'),
                   ('key-turkish-i', '{"env": {"ANTHROPıC_BASE_URL": "x", "ANTHROPİC_API_KEY": "y", "CLAUDE_CONFıG_DİR": "z"}}'),
                   ('key-long-s-kelvin', '{"env": {"HTTPſ_PROXY": "x", "ANTHROPIC_API_KEY": "y", "CLAUDE_CODE_UſE_BEDROCK": "z"}}'),
                   ('key-fullwidth', '{"env": {"ＨＴＴＰＳ_PROXY": "x", "HTTPS＿PROXY": "y"}}'),
                   ('env-nested', '{"env": {"HTTPS_PROXY": {"a": 1}, "NO_PROXY": null, "ALL_PROXY": [1]}}'),
                   ('big-int', '{"n": %s, "env": {"NO_PROXY": "x"}}' % ('9' * 4300)), ('int-too-long', '{"n": %s, "env": {"NO_PROXY": "x"}}' % ('9' * 4301)),
                   ('negative-int-too-long', '{"n": [1, -%s]}' % ('1' * 4301)), ('float-many-digits', '{"n": %s.5, "m": 1e%s, "k": 0.%s}' % ('9' * 5000, '1' * 4400, '3' * 5000)),
                   ('long-digits-in-string', '{"n": "%s", "env": {}}' % ('9' * 5000)),
                   ('nested-900', '{"x": %s%s}' % ('[' * 900, ']' * 900)), ('nested-too-deep', '{"x": %s%s}' % ('[' * 300000, ']' * 300000)),
                   ('nested-too-deep-objects', '{"x": %s1%s, "env": {"NO_PROXY": "x"}}' % ('{"a": ' * 300000, '}' * 300000)),
                   ('brackets-in-string', json.dumps({'x': '[' * 5000 + '{' * 5000 + '\\"[[[', 'env': {'NO_PROXY': 'x'}}))]:
    @case('settings-check/claude-json-%s' % name, ['settings-check'])
    def _(b, text=text):
        b.write('.claude/settings.json', text)


@case('settings-check/claude-not-utf8', ['settings-check'])
def _(b):
    b.write('.claude/settings.json', b'{"env": {"HTTPS_PROXY": "\xff"}}')
    b.write('.claude/settings.local.json', b'\xed\xa0\x80')


@case('settings-check/claude-is-directory', ['settings-check'])
def _(b):
    mkdir(b, '.claude/settings.json')
    settings(b, {'env': {'NO_PROXY': 'x'}}, 'settings.local.json')


@case('settings-check/claude-dangling-link', ['settings-check'])
def _(b):
    link(b, '.claude/settings.json', b.path('missing-settings.json'))
    link(b, '.claude/settings.local.json', 'missing-relative.json')


@case('settings-check/claude-link-loop', ['settings-check'])
def _(b):
    link(b, '.claude/settings.json', 'settings.json')


@case('settings-check/claude-linked-to-dotfiles', ['settings-check'])
def _(b):
    b.json('dotfiles/claude.json', {'env': {'HTTPS_PROXY': 'http://example.invalid'}, 'apiKeyHelper': 'x'})
    link(b, '.claude/settings.json', b.path('dotfiles/claude.json'))
    b.json('dotfiles/local.json', {'env': {}})
    link(b, '.claude/settings.local.json', '../dotfiles/local.json')


@case('settings-check/claude-master-is-file', ['settings-check'])
def _(b):
    b.write('.claude', 'not a directory')


@case('settings-check/claude-master-is-dangling-link', ['settings-check'])
def _(b):
    link(b, '.claude', b.path('nowhere'))


@case('settings-check/claude-master-is-link', ['settings-check'])
def _(b):
    b.json('real-claude/settings.json', {'env': {'ALL_PROXY': 'x'}})
    link(b, '.claude', 'real-claude')


@case('settings-check/claude-ignores-other-tools', ['settings-check'])
def _(b):
    toml(b, 'chatgpt_base_url = "https://example.invalid"')
    agy(b, {'gcp': {'project': 'p'}})


# ---- settings-check：Codex 的 config.toml、profile、.env -------------------------------------------------------------

BENIGN_TOML = '''model = "sample-model"
model_reasoning_effort = "ultra"
service_tier = "priority"
text = """A note containing chatgpt_base_url = "example"
features = { respect_system_proxy = true }
"""
when = 2026-11-08T12:00:00Z
choices = [1, 2, 3]
model_provider = "openai"
cli_auth_credentials_store = "file"
forced_login_method = "chatgpt"
profile = "work 配置"
[features]
respect_system_proxy = false
system_proxy_fallback = false
other_feature = true
[projects."/tmp/test-project"]
trust_level = "trusted"
[model_providers.custom]
base_url = "https://example.invalid/v1"
[profiles.safe]
model = "m"
[[sample]]
name = "one"
'''

CODEX_TOML = [
    ('none', None), ('empty', ''), ('benign', BENIGN_TOML),
    # tests/isolation_settings.py：多行字符串、转义的键、带点的键、行内表里的敏感键
    ('multiline-basic', 'chatgpt_base_url = """\nhttps://example.invalid"""\n'), ('multiline-literal', "chatgpt_base_url = '''\nhttps://example.invalid'''\n"),
    ('quoted-key', '"chatgpt_base_url" = "https://example.invalid"\n'), ('escaped-key', '"\\u0063hatgpt_base_url" = "https://example.invalid"\n'),
    ('profile-table', '[profiles.other]\nchatgpt_base_url = "https://example.invalid"\n'),
    ('profile-dotted', 'profiles.other.chatgpt_base_url = "https://example.invalid"\n'),
    ('profile-inline', 'profiles = { other = { chatgpt_base_url = "https://example.invalid" } }\n'),
    ('features-inline', 'features = { respect_system_proxy = true }'), ('features-dotted', 'features.respect_system_proxy = true'),
    ('features-quoted', '["features"]\n"respect_system_proxy" = true'),
    ('features-in-quoted-profile', 'profiles = { "deep.review" = { features = { respect_system_proxy = true } } }'),
    ('literal-dotted-key-is-not-nested', '"features.respect_system_proxy" = true\n"profiles.x.chatgpt_base_url" = "u"\n"model_providers.openai" = 1'),
    ('unused-profile', 'profile = "safe"\n[profiles.safe]\nmodel="m"\n[profiles."unused.review"]\ncli_auth_credentials_store="keyring"'),
    ('builtin-provider', '[model_providers.openai]\nbase_url="https://example.invalid"'),
    ('builtin-provider-dotted', 'model_providers.openai.name = "x"\nmodel_providers.other.name = "y"'),
    ('profile-reenables-fallback', 'profile="x"\n[features]\nsystem_proxy_fallback=false\n[profiles.x.features]\nsystem_proxy_fallback=true\n'),
    ('base-fallback-on', '[features]\nsystem_proxy_fallback=true\n'), ('base-fallback-off', '[features]\nsystem_proxy_fallback=false\nrespect_system_proxy=false\n'),
    # 每个不许有的键
    ('every-bad-key', 'forced_chatgpt_workspace_id = "w"\nexperimental_realtime_webrtc_call_base_url = "u"\nexperimental_realtime_ws_base_url = "u"\n'
                      'chatgpt_base_url = "u"\nopenai_base_url = "u"\nmodel = "m"'),
    ('bad-keys-any-type', 'openai_base_url = 1\nchatgpt_base_url = false\nexperimental_realtime_ws_base_url = []\nforced_chatgpt_workspace_id = {}'),
    ('must-be-wrong', 'model_provider = "custom"\ncli_auth_credentials_store = "keyring"\nforced_login_method = "api"'),
    ('must-be-right', 'model_provider = "openai"\ncli_auth_credentials_store = "file"\nforced_login_method = "chatgpt"'),
    ('must-be-other-types', 'model_provider = ["openai"]\ncli_auth_credentials_store = true\nforced_login_method = ["api"]'),
    ('must-be-case', 'model_provider = "OpenAI"\ncli_auth_credentials_store = "File"\nforced_login_method = "API"'),
    ('must-be-numbers', 'model_provider = 1\ncli_auth_credentials_store = 1.5\nforced_login_method = 0'),
    ('features-not-table', 'features = "not a table"'), ('features-list', 'features = [{respect_system_proxy = true}]'), ('features-empty', '[features]'),
    ('features-not-bool', '[features]\nrespect_system_proxy = "false"\nsystem_proxy_fallback = 0'),
    ('features-not-bool-2', '[features]\nrespect_system_proxy = 1\nsystem_proxy_fallback = []'),
    ('features-floats', '[features]\nrespect_system_proxy = 0.0\nsystem_proxy_fallback = nan'),
    ('features-both-on', '[features]\nrespect_system_proxy = true\nsystem_proxy_fallback = true'),
    ('features-both-off', 'features = { respect_system_proxy = false, system_proxy_fallback = false }'),
    ('model-providers-not-table', 'model_providers = "x"'), ('model-providers-list', 'model_providers = [1]'), ('model-providers-other', '[model_providers.mine]\nbase_url = "u"'),
    ('profiles-not-table', 'profiles = []'), ('profiles-string', 'profiles = "x"'), ('profile-not-table', 'profiles.a = 1\nprofiles.b = "x"\nprofiles.c = []'),
    ('profiles-empty', '[profiles]'), ('profiles-many', '[profiles.b]\nopenai_base_url = "u"\n[profiles.a]\nmodel_provider = "x"\n[profiles."c d"]\nforced_login_method = "api"\n'
                                                   '[profiles.e.features]\nsystem_proxy_fallback = true\nrespect_system_proxy = true\n[profiles.f.model_providers.openai]\nx = 1'),
    ('profiles-nested', '[profiles.a.profiles.b.profiles.c]\nchatgpt_base_url = "u"\n[profiles.a.profiles.b]\nprofile = "../x"'),
    ('profile-in-profile-fallback', '[features]\nsystem_proxy_fallback = true\n[profiles.a.features]\nsystem_proxy_fallback = false\n[profiles.b.features]\nsystem_proxy_fallback = true'),
    # profile 的名字
    ('profile-name-ok', 'profile = "deep-review"'), ('profile-name-unicode', 'profile = "工作 配置"'), ('profile-name-dots', 'profile = "review.v2"'),
    ('profile-name-escape', 'profile = "../outside"'), ('profile-name-absolute', 'profile = "/tmp/outside"'), ('profile-name-nested', 'profile = "nested/profile"'),
    ('profile-name-hidden', 'profile = ".hidden"'), ('profile-name-empty', 'profile = ""'), ('profile-name-newline', 'profile = "bad\\nname"'),
    ('profile-name-tab', 'profile = "bad\\tname"'), ('profile-name-del', 'profile = "bad\\u007fname"'), ('profile-name-nul', 'profile = "bad\\u0000name"'),
    ('profile-name-number', 'profile = 123'), ('profile-name-bool', 'profile = true'), ('profile-name-float', 'profile = 1.5'), ('profile-name-list', 'profile = ["a"]'),
    ('profile-name-table', 'profile = { name = "a" }'), ('profile-name-date', 'profile = 2026-11-08'), ('profile-name-time', 'profile = 12:00:00'),
    ('profile-name-space', 'profile = " "'), ('profile-name-nbsp', 'profile = "a\\u00a0b\\u0085c"'), ('profile-name-dot', 'profile = "."'),
    # 日期、时间出现在要看类型的地方
    ('dates-everywhere', 'model_provider = 2026-11-08\ncli_auth_credentials_store = 12:00:00\nforced_login_method = 2026-11-08T12:00:00Z\n'
                         'features = 2026-11-08\nmodel_providers = 2026-11-08\nprofiles = 2026-11-08'),
    ('dates-in-features', '[features]\nrespect_system_proxy = 2026-11-08\nsystem_proxy_fallback = 12:00:00.5\n[profiles]\na = 2026-11-08T12:00:00+08:00'),
    # 解析不了的
    ('unterminated', 'model="unterminated'), ('duplicate', 'model="a"\nmodel="b"'), ('bad-date', 'when = 2026-02-30'), ('bom', '﻿model = "m"'),
    ('table-twice', '[features]\n[features]'), ('invalid-escape', 'model = "\\x41"'), ('surrogate-escape', 'model = "\\ud800"'), ('bare-value', 'model = sample'),
    ('trailing-garbage', 'model = "a" b'), ('int-too-long', 'n = ' + '9' * 4301), ('int-4300', 'n = ' + '9' * 4300), ('control-char', 'model = "a\x01"'),
    ('inline-trailing-comma', 'features = { respect_system_proxy = false, }'), ('toml-1.1-escape', 'model = "\\e"'),
    ('hidden-behind-error', 'model = "a"\nchatgpt_base_url = "u"\nmodel = "b"'),
    # 换行
    ('crlf', 'chatgpt_base_url = "u"\r\n[features]\r\nrespect_system_proxy = true\r\n'), ('cr-only', 'model = "a"\rchatgpt_base_url = "u"\r'),
    ('cr-in-string', 'model = """a\rb"""\nopenai_base_url = "u"'),
    # 套得深的：行内数组、行内表（300 层以内两边都行，600 层两边都是 RecursionError）
    ('nested-arrays-300', 'a = ' + '[' * 300 + ']' * 300 + '\nopenai_base_url = "u"'), ('nested-arrays-600', 'a = ' + '[' * 600 + ']' * 600),
    ('nested-tables-250', 'a = ' + '{a = ' * 250 + '1' + '}' * 250 + '\nopenai_base_url = "u"'), ('nested-tables-400', 'a = ' + '{a = ' * 400 + '1' + '}' * 400),
    ('nested-arrays-unclosed', 'openai_base_url = "u"\na = ' + '[' * 5000),
    # 刚好在上限两边的（Python 的调用栈一千层：settings-check 这条路上数组能套 494 层、行内表 329 层、profiles 994 层）
    ('nested-arrays-494', 'a = ' + '[' * 494 + ']' * 494 + '\nopenai_base_url = "u"'), ('nested-arrays-495', 'a = ' + '[' * 495 + ']' * 495),
    ('nested-tables-329', 'a = ' + '{a = ' * 329 + '1' + '}' * 329 + '\nopenai_base_url = "u"'), ('nested-tables-330', 'a = ' + '{a = ' * 330 + '1' + '}' * 330),
    ('profiles-994-deep', 'profiles.a.' * 994 + 'chatgpt_base_url = "u"'), ('profiles-995-deep', 'profiles.a.' * 995 + 'chatgpt_base_url = "u"'),
    # profiles 一层套一层
    ('profiles-100-deep', 'profiles.a.' * 100 + 'chatgpt_base_url = "u"'), ('profiles-2000-deep', 'profiles.a.' * 2000 + 'chatgpt_base_url = "u"'),
    ('dotted-key-2000-deep', '.'.join(['k'] * 2000) + ' = 1\nopenai_base_url = "u"'),
]

for name, text in CODEX_TOML:
    @case('settings-check/codex-%s' % name, ['settings-check'], tool='codex')
    def _(b, text=text):
        if text is not None:
            toml(b, text)


@case('settings-check/codex-not-utf8', ['settings-check'], tool='codex')
def _(b):
    b.write('.codex/config.toml', b'\xff')


@case('settings-check/codex-is-directory', ['settings-check'], tool='codex')
def _(b):
    mkdir(b, '.codex/config.toml')


@case('settings-check/codex-dangling-link', ['settings-check'], tool='codex')
def _(b):
    link(b, '.codex/config.toml', b.path('missing-config'))


@case('settings-check/codex-link-loop', ['settings-check'], tool='codex')
def _(b):
    link(b, '.codex/config.toml', 'config.toml')
    link(b, '.codex/.env', '.env')


@case('settings-check/codex-linked-to-dotfiles', ['settings-check'], tool='codex')
def _(b):
    b.write('dotfiles/codex.toml', 'openai_base_url = "https://example.invalid"')
    link(b, '.codex/config.toml', '../dotfiles/codex.toml')
    b.write('dotfiles/p.toml', 'features.system_proxy_fallback = true')
    link(b, '.codex/linked.config.toml', b.path('dotfiles/p.toml'))


@case('settings-check/codex-master-is-file', ['settings-check'], tool='codex')
def _(b):
    b.write('.codex', 'not a directory')


@case('settings-check/codex-master-is-dangling-link', ['settings-check'], tool='codex')
def _(b):
    link(b, '.codex', b.path('nowhere'))


@case('settings-check/codex-master-is-link', ['settings-check'], tool='codex')
def _(b):
    b.write('real-codex/config.toml', 'chatgpt_base_url = "u"')
    b.write('real-codex/x.config.toml', 'openai_base_url = "u"')
    b.write('real-codex/.env', 'CODEX_HOME=/x\n')
    link(b, '.codex', 'real-codex')


@case('settings-check/codex-profile-files', ['settings-check'], tool='codex')
def _(b):
    toml(b, 'model="m"')
    toml(b, 'forced_chatgpt_workspace_id="test-workspace"', 'unused.config.toml')
    toml(b, 'features={system_proxy_fallback=true}\n', 'deep.config.toml')
    toml(b, 'model = "sample-model"\n', '工作 配置.config.toml')
    toml(b, 'chatgpt_base_url = "u"', 'Zed.config.toml')
    toml(b, 'chatgpt_base_url = "u"', 'a b.config.toml')
    toml(b, 'chatgpt_base_url = "u"', '~.config.toml')
    toml(b, 'chatgpt_base_url = "u"', 'é.config.toml')
    toml(b, 'chatgpt_base_url = "u"', '😀.config.toml')
    toml(b, 'chatgpt_base_url = "u"', 'Ａ.config.toml')
    toml(b, 'chatgpt_base_url = "u"', '.hidden.config.toml')     # 点开头的不算 profile
    toml(b, 'chatgpt_base_url = "u"', '.config.toml')
    toml(b, 'chatgpt_base_url = "u"', 'notes.toml')
    toml(b, 'chatgpt_base_url = "u"', 'x.config.toml.bak')
    toml(b, 'chatgpt_base_url = "u"', 'sub/inner.config.toml')    # 子目录里的不算
    toml(b, 'model = "unterminated', 'broken.config.toml')
    toml(b, 'profile = "x"\n[profiles.y]\nopenai_base_url = "u"', 'nested.config.toml')
    mkdir(b, '.codex/dir.config.toml')
    link(b, '.codex/gone.config.toml', 'nowhere.toml')
    b.write('.codex/binary.config.toml', b'\xff\xfe')


@case('settings-check/codex-profile-file-only', ['settings-check'], tool='codex')
def _(b):
    toml(b, 'cli_auth_credentials_store = "keyring"', 'only.config.toml')


for name, text in [('access-token', 'export CODEX_ACCESS_TOKEN=dummy-do-not-echo\n'), ('api-key', 'CODEX_API_KEY=dummy-do-not-echo'), ('home', 'CODEX_HOME=/tmp/x'),
                   ('base-url', 'CODEX_CHATGPT_BASE_URL_OVERRIDE=https://example.invalid'), ('openai-key', 'OPENAI_API_KEY=dummy-do-not-echo'),
                   ('lower-proxy', 'https_proxy=http://example.invalid'), ('ca-cert', 'CODEX_CA_CERTIFICATE=/tmp/ca.pem\nEDITOR=vi\n'),
                   ('many', '# CODEX_HOME=commented\n\n  export   HTTP_PROXY = x\nALL_PROXY=x\nNO_PROXY=x\nall_proxy=y\nOPENAI_X=1\nopenai_base_url=x\nCODEX_=1\nOPENAI_=1\n'
                            'CODEX=1\nXCODEX_HOME=1\nCODEX_HOME =2\nCODEX_HOME=3\nexport=1\nexportCODEX_A=1\nexport\tCODEX_B=1\nexport CODEX_C\nCODEX_D\n=CODEX_E\n'
                            'CODEX_CA_CERTIFICATE=1\ncodex_ca_certificate=2\nCODEX_CA_CERTIFICATE2=3\n1CODEX_F=1\n_CODEX_G=1\nCODEX_Ｈ=1\nCODEX_I=a=b\n　CODEX_J=1\n'
                            'export export=1\nexport  = 1\nexport CODEX_K = 1 # c\nPATH=$PATH:CODEX_L=1\n'),
                   ('line-breaks', 'A=1\rCODEX_M=1\r\nCODEX_N=1\x0bCODEX_O=1\x0cCODEX_P=1\x1cCODEX_Q=1\x1dCODEX_R=1\x1eCODEX_S=1\x85CODEX_T=1 CODEX_U=1 CODEX_V=1'),
                   ('repeated', 'CODEX_HOME=1\nCODEX_HOME=2\nexport CODEX_HOME=3\n'), ('empty', ''), ('no-newline-at-end', 'x=1\nOPENAI_BASE_URL=u'),
                   ('bom', '﻿CODEX_HOME=1\nCODEX_HOME2=1'), ('unicode-space', ' CODEX_W=1\n export CODEX_X =1\n\x1fCODEX_Y=1')]:
    @case('settings-check/codex-env-%s' % name, ['settings-check'], tool='codex')
    def _(b, text=text):
        b.write('.codex/.env', text)


@case('settings-check/codex-env-is-directory', ['settings-check'], tool='codex')
def _(b):
    mkdir(b, '.codex/.env')


@case('settings-check/codex-env-not-utf8', ['settings-check'], tool='codex')
def _(b):
    b.write('.codex/.env', b'CODEX_HOME=\xff\n')


@case('settings-check/codex-env-dangling-link', ['settings-check'], tool='codex')
def _(b):
    link(b, '.codex/.env', 'nowhere')


@case('settings-check/codex-everything-wrong', ['settings-check'], tool='codex')
def _(b):
    toml(b, 'chatgpt_base_url = "u"\nprofile = 1\nmodel_provider = "x"\n[features]\nrespect_system_proxy = true\n[profiles.p]\nopenai_base_url = "u"')
    toml(b, 'chatgpt_base_url = "u"', 'a.config.toml')
    toml(b, 'x = ', 'b.config.toml')
    b.write('.codex/.env', 'OPENAI_API_KEY=dummy\nHTTPS_PROXY=x\nOPENAI_API_KEY=dummy2\n')


@case('settings-check/codex-ignores-other-tools', ['settings-check'], tool='codex')
def _(b):
    settings(b, {'env': {'HTTPS_PROXY': 'x'}})
    agy(b, {'gcp': 1})


# ---- settings-check：agy 的 antigravity-cli/settings.json ------------------------------------------------------------

for name, text in [('none', None), ('clean', '{"theme": "dark", "gcpx": 1, "GCP": 2, "nested": {"gcp": 1}}'), ('gcp', '{"gcp": {"project": "p", "location": "l"}}'),
                   ('gcp-null', '{"gcp": null}'), ('dotted', '{"gcp.project": "p"}'), ('dotted-location', '{"a": 1, "gcp.location": "l"}'),
                   ('all', '{"gcp.location": "l", "gcp.project": "p", "gcp": {}}'), ('list', '[]'), ('string', '"gcp"'), ('null', 'null'), ('number', '0'),
                   ('bad-json', '{"gcp": '), ('empty', ''), ('bom', '﻿{}'), ('crlf', '{\r\n"gcp": 1\r\n}'),
                   ('nested-too-deep', '{"x": %s%s}' % ('[' * 300000, ']' * 300000)), ('nested-900', '{"gcp": %s%s}' % ('[' * 900, ']' * 900)),
                   ('int-too-long', '{"x": %s}' % ('7' * 4301)), ('duplicate', '{"gcp": 1, "gcp": 2}')]:
    @case('settings-check/agy-%s' % name, ['settings-check'], tool='agy')
    def _(b, text=text):
        if text is not None:
            b.write('.gemini/antigravity-cli/settings.json', text)


@case('settings-check/agy-not-utf8', ['settings-check'], tool='agy')
def _(b):
    b.write('.gemini/antigravity-cli/settings.json', b'{"gcp": "\xff"}')


@case('settings-check/agy-is-directory', ['settings-check'], tool='agy')
def _(b):
    mkdir(b, '.gemini/antigravity-cli/settings.json')


@case('settings-check/agy-dangling-link', ['settings-check'], tool='agy')
def _(b):
    link(b, '.gemini/antigravity-cli/settings.json', 'nowhere.json')     # 没有这个文件：和别处不一样，这里不算读不了


@case('settings-check/agy-link-loop', ['settings-check'], tool='agy')
def _(b):
    link(b, '.gemini/antigravity-cli/settings.json', 'settings.json')


@case('settings-check/agy-parent-is-file', ['settings-check'], tool='agy')
def _(b):
    b.write('.gemini/antigravity-cli', 'not a directory')


@case('settings-check/agy-linked', ['settings-check'], tool='agy')
def _(b):
    b.json('dotfiles/agy.json', {'gcp.project': 'p'})
    link(b, '.gemini/antigravity-cli/settings.json', b.path('dotfiles/agy.json'))


@case('settings-check/agy-ignores-other-tools', ['settings-check'], tool='agy')
def _(b):
    settings(b, {'env': {'HTTPS_PROXY': 'x'}})
    toml(b, 'chatgpt_base_url = "u"')


# ---- launch-check：Claude 的启动参数、项目设置 ---------------------------------------------------------------------

BAD_JSON = '{"env":{"ANTHROPIC_BASE_URL":"https://example.invalid","EDITOR":"vi"},"apiKeyHelper":"echo dummy"}'

CLAUDE_ARGS = [
    ('none', []), ('prompt', ['write a test', '-p']), ('settings-inline-bad', ['--settings', '{"env":{"ANTHROPIC_AUTH_TOKEN":"dummy"}}']),
    ('settings-inline-ok', ['--settings', '{"model":"sample-model"}']), ('settings-inline-equals', ['--settings=' + BAD_JSON]),
    ('settings-inline-leading-space', ['--settings', ' \n\t' + BAD_JSON]), ('settings-inline-unicode-space', ['--settings', '　' + BAD_JSON]),
    ('settings-inline-vt', ['--settings', '\x0b{"env":{}}']), ('settings-inline-invalid', ['--settings', '{"env":']),
    ('settings-inline-env-list', ['--settings', '{"env":[]}']), ('settings-inline-not-object', ['--settings', '[]']),
    # 套得太深（一个参数在 Linux 上最多 128KB：只写开头的十万个括号，没合上；不管先撞上哪个上限都是不合法）
    ('settings-inline-too-deep', ['--settings', '{"x":' + '[' * 100000]), ('settings-inline-deep-ok', ['--settings', '{"x":' + '[' * 900 + ']' * 900 + ',"env":{"NO_PROXY":"x"}}']),
    ('settings-inline-keys-with-newline', ['--settings', '{"env":{"HTTPS_PROXY\\n":"x"}}']),
    ('settings-twice', ['--settings', '{"env":{"NO_PROXY":"x"}}', '--settings={"apiKeyHelper":"x","env":{"NO_PROXY":"y"}}']),
    ('settings-missing-value', ['--settings']), ('settings-empty', ['--settings=']), ('settings-empty-next', ['--settings', '']),
    ('settings-missing-file', ['--settings', 'missing.json']), ('settings-file', ['--settings=custom.json']), ('settings-file-next', ['--settings', 'custom.json']),
    ('settings-file-dot', ['--settings', './sub/../custom.json']), ('settings-file-clean', ['--settings', 'clean.json']),
    ('settings-file-broken', ['--settings', 'broken.json']), ('settings-file-directory', ['--settings', 'sub']), ('settings-file-list', ['--settings', 'list.json']),
    ('settings-file-home', ['--settings', '~/home-settings.json']), ('settings-file-home-missing', ['--settings=~/nope.json']),
    ('settings-file-tilde-only', ['--settings', '~']), ('settings-file-unknown-user', ['--settings', '~no-such-user-cca-test/x.json']),
    ('settings-file-etc', ['--settings', '/etc/hosts']), ('settings-file-dangling', ['--settings', 'dangling.json']),
    ('settings-file-project-like', ['--settings', 'other/.claude/settings.json']),
    ('desktop', ['--desktop']), ('desktop-equals', ['--desktop=1']), ('desktop-twice', ['--desktop', '-p', '--desktop']),
    # 必填参数的值恰好是 -- 或 --settings：先被吃掉
    ('value-is-settings', ['--system-prompt', '--settings']), ('value-is-dashes', ['--system-prompt', '--', '--settings', BAD_JSON]),
    ('value-is-desktop', ['--system-prompt', '--desktop']), ('after-dashes', ['--', '--settings', BAD_JSON, '--desktop']),
    ('model-then-settings', ['--model', 'x', '--settings', BAD_JSON]), ('n-takes-value', ['-n', '--desktop', '--desktop']),
    ('equals-form-takes-no-next', ['--model=x', '--desktop']), ('missing-value-model', ['--settings', BAD_JSON, '--model']),
    ('missing-value-short', ['-n']), ('unknown-options', ['--frobnicate', '--settings2', BAD_JSON, '-x=--settings', '=', '']),
    ('every-value-option', sum([[o, '--desktop'] for o in [
        '--agent', '--agents', '--append-system-prompt', '--autocompact', '--debug-file', '--effort', '--environment', '--fallback-model', '--input-format',
        '--json-schema', '--max-budget-usd', '--model', '-n', '--name', '--output-format', '--permission-mode', '--permission-prompts', '--plugin-dir',
        '--plugin-url', '--remote-control-session-name-prefix', '--session-id', '--setting-sources', '--system-prompt', '--system-prompt-snapshot',
        '--system-prompt-file', '--append-system-prompt-file', '--add-dir', '--allowedTools', '--allowed-tools', '--betas', '--disallowedTools',
        '--disallowed-tools', '--file', '--mcp-config', '--tools']], []) + ['--verbose', '--desktop']),
    ('codex-flags-mean-nothing', ['-c', 'chatgpt_base_url=x', '--remote', 'x', '--oss', 'app', '--gemini_dir', 'x', '-C', 'other']),
]


def claude_project(b):
    """~/project 是工作目录；~/other 是旁边的项目（不该被查到）"""
    b.write('project/custom.json', '{"env":{"HTTPS_PROXY":"http://example.invalid"},"apiKeyHelper":"x"}')
    b.write('project/clean.json', '{"permissions":{"allow":["Read"]}}')
    b.write('project/broken.json', '{"env":')
    b.write('project/list.json', '[]')
    mkdir(b, 'project/sub')
    link(b, 'project/dangling.json', 'nowhere.json')
    b.write('home-settings.json', '{"env":{"ALL_PROXY":"x"}}')
    b.write('other/.claude/settings.json', '{"env":{"NO_PROXY":"x"}}')
    b.write('project/other/.claude/settings.json', '{"env":{"NO_PROXY":"x"}}')
    return {'cwd': 'project'}


# 这些参数的说明（settings-explain launch）也对一遍：每种说法至少一个
EXPLAINED = {'none', 'settings-inline-bad', 'settings-inline-invalid', 'settings-inline-env-list', 'settings-inline-not-object', 'settings-inline-keys-with-newline',
             'settings-twice', 'settings-missing-value', 'settings-file', 'settings-file-broken', 'settings-file-home', 'settings-file-etc', 'settings-file-dangling',
             'settings-file-project-like', 'settings-file-tilde-only', 'desktop', 'desktop-twice', 'value-is-dashes', 'missing-value-model', 'every-value-option'}

for name, args in CLAUDE_ARGS:
    @case('launch-check/claude-%s' % name, ['launch-check'] + args)
    def _(b):
        return claude_project(b)

    if name in EXPLAINED:
        @case('settings-explain/claude-launch-%s' % name, ['settings-explain', 'launch'] + args)
        def _(b):
            return claude_project(b)


# 换掉凭证来源的那几个变量：共享的、项目里的、--settings 给的都不行（tests/isolation_settings.py）
CREDENTIAL_SOURCES = {n: 'private-fixture-value' for n in (
    'CLAUDE_CONFIG_DIR', 'CLAUDE_SECURESTORAGE_CONFIG_DIR', 'CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR', 'CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR',
    'CLAUDE_CODE_GATEWAY_TOKEN_FILE_DESCRIPTOR', 'CLAUDE_CODE_WEBSOCKET_AUTH_FILE_DESCRIPTOR')}


@case('launch-check/claude-credential-sources', ['launch-check', '--settings', json.dumps({'env': CREDENTIAL_SOURCES})])
def _(b):
    settings(b, {'env': CREDENTIAL_SOURCES})
    b.json('project/.claude/settings.local.json', {'env': CREDENTIAL_SOURCES})
    return {'cwd': 'project'}


@case('launch-check/claude-shared-and-args', ['launch-check', '--desktop', '--settings', BAD_JSON, '--model'])
def _(b):
    settings(b, {'env': {'NO_PROXY': 'x'}})
    b.write('project/.claude/settings.local.json', '{"apiKeyHelper":"x"}')
    return {'cwd': 'project'}


def claude_tree(b):
    """项目设置在工作目录和各级上层目录里；旁边的、下层的目录不查"""
    b.write('work/.claude/settings.json', '{"env":{"ANTHROPIC_BASE_URL":"https://example.invalid"}}')
    b.write('work/.claude/settings.local.json', '{"env":{"HTTPS_PROXY":"x"},"apiKeyHelper":"x"}')
    b.write('work/repo/.claude/settings.local.json', '{"env":{"CLAUDE_CONFIG_DIR":"/x"}}')
    b.write('work/repo/pkg/.claude/settings.json', '{broken')
    b.write('work/repo/pkg/src/deep/.claude/settings.json', '{"env":{"NO_PROXY":"x"}}')     # 更下层的
    b.write('work/repo/other/.claude/settings.json', '{"env":{"NO_PROXY":"x"}}')            # 旁边的
    b.write('work/repo/pkg/src/.claude/keybindings.json', '{"env":{"NO_PROXY":"x"}}')       # 别的文件
    mkdir(b, 'work/repo/pkg/src')


for name, rel in [('in-project', 'work/repo/pkg/src'), ('one-up', 'work/repo/pkg'), ('repo', 'work/repo'), ('top', 'work'), ('sibling', 'work/repo/other'),
                  ('deeper', 'work/repo/pkg/src/deep'), ('inside-dot-claude', 'work/repo/.claude')]:
    @case('launch-check/claude-project-%s' % name, ['launch-check'])
    def _(b, rel=rel):
        claude_tree(b)
        return {'cwd': rel}

    @case('settings-explain/claude-project-%s' % name, ['settings-explain', 'launch'])
    def _(b, rel=rel):
        claude_tree(b)
        return {'cwd': rel}


def claude_unreadable_tree(b):
    mkdir(b, 'p/.claude/settings.json')
    link(b, 'p/.claude/settings.local.json', 'nowhere')
    b.write('p/q/.claude/settings.json', b'\xff')
    b.write('p/q/.claude/settings.local.json', '[]')
    b.write('p/q/r/.claude', 'a file, not a directory')
    b.write('p/q/r/s/.claude/settings.json', '{"env": 1, "apiKeyHelper": "x"}')
    b.write('p/q/r/s/.claude/settings.local.json', '{"env": {"NO_PROXY": "x"}}')
    return {'cwd': 'p/q/r/s'}


@case('launch-check/claude-project-unreadable', ['launch-check'])
def _(b):
    return claude_unreadable_tree(b)


@case('settings-explain/claude-project-unreadable', ['settings-explain', 'launch'])
def _(b):
    return claude_unreadable_tree(b)


@case('launch-check/claude-home-is-cwd', ['launch-check'])
def _(b):
    # 工作目录就是主目录：~/.claude/settings.json 已经当共享的查过，不再当项目设置查一遍
    settings(b, {'env': {'NO_PROXY': 'x'}})
    settings(b, {'apiKeyHelper': 'x'}, 'settings.local.json')


@case('launch-check/claude-home-is-cwd-linked', ['launch-check'])
def _(b):
    # 共享的那份是指向别处的链接：解开后的路径和 ~/.claude/settings.json 不一样，会再按项目设置查到一遍
    b.json('dotfiles/claude.json', {'env': {'NO_PROXY': 'x'}})
    link(b, '.claude/settings.json', b.path('dotfiles/claude.json'))
    return {'cwd': 'sub/dir'}


@case('launch-check/claude-cwd-is-linked-dir', ['launch-check', '--settings', 'here.json'])
def _(b):
    b.write('real/proj/.claude/settings.json', '{"env":{"NO_PROXY":"x"}}')
    b.write('real/proj/here.json', '{"env":{"ALL_PROXY":"x"}}')
    link(b, 'alias', 'real/proj')
    return {'cwd': 'alias'}


@case('launch-check/claude-master-inside-project', ['launch-check'])
def _(b):
    settings(b, {'env': {'NO_PROXY': 'x'}})
    return {'cwd': '.claude'}


# ---- launch-check：Codex 的启动参数、项目配置 ----------------------------------------------------------------------

CODEX_ARGS = [
    ('none', []), ('benign', ['-c', 'model=sample-model', '-cmodel_reasoning_effort="ultra"', '--disable', 'respect_system_proxy', '--profile', 'deep-review']),
    ('c-base-url', ['-c', 'chatgpt_base_url=https://example.invalid']), ('config-equals', ['--config=chatgpt_base_url="https://example.invalid"']),
    ('c-attached', ['-cmodel_provider=custom']), ('c-attached-equals', ['-c=model_provider=openai']), ('c-provider-openai', ['--config', 'model_provider="openai"']),
    ('c-features-inline', ['-c', 'features={respect_system_proxy=true}']), ('c-features-dotted', ['-c', 'features.system_proxy_fallback=true']),
    ('c-features-off', ['-c', 'features.system_proxy_fallback=false', '-c', 'features.respect_system_proxy=false']),
    ('c-features-not-bool', ['-c', 'features.respect_system_proxy=off']), ('c-features-not-table', ['-c', 'features=on']),
    ('c-store', ['-c', 'cli_auth_credentials_store="file"']), ('c-login-method', ['-c', 'forced_login_method=chatgpt']),
    ('c-profile-ok', ['-c', 'profile="deep-review"']), ('c-profile-bare', ['-c', 'profile=deep-review']), ('c-profile-number', ['-c', 'profile=123']),
    ('c-profile-empty', ['-c', 'profile=']), ('c-profile-escape', ['-c', 'profile="../outside"']), ('c-profile-newline', ['-c', 'profile="bad\\nname"']),
    ('c-profile-absolute', ['-c', 'profile="/tmp/outside"']), ('c-profile-nested', ['-c', 'profile="nested/profile"']), ('c-profile-hidden', ['-c', 'profile=".hidden"']),
    ('c-profile-unicode', ['-c', 'profile=工作 配置']), ('c-profile-list', ['-c', 'profile=["a"]']), ('c-profile-date', ['-c', 'profile=2026-11-08']),
    ('c-profiles', ['-c', 'profiles.a.chatgpt_base_url=x', '-c', 'profiles.b.model=m', '--config', 'profiles={c={features={system_proxy_fallback=false}}}']),
    ('c-profiles-not-table', ['-c', 'profiles=1', '-c', 'profiles.a=1']), ('c-provider', ['-c', 'model_providers.openai.base_url=x', '-c', 'model_providers.mine.base_url=x']),
    ('c-no-equals', ['-c', 'model']), ('c-missing', ['-c']), ('config-missing', ['exec', '--config']), ('c-empty', ['-c', '']), ('c-only-equals', ['-c', '=']),
    ('c-attached-only-equals', ['-c=']), ('c-double-equals', ['-c==x']), ('c-empty-key', ['-c', '=value']), ('c-bad-key', ['-c', 'a b=1']),
    ('c-quoted-key', ['-c', '"chatgpt_base_url"=x', '-c', "'openai_base_url'=1", '-c', '"a b"=1']), ('c-escaped-key', ['-c', '"\\u0063hatgpt_base_url"=x']),
    ('c-value-with-equals', ['-c', 'model=a=b', '-c', 'chatgpt_base_url=https://example.invalid/?a=b']), ('c-multiline', ['-c', 'model="a"\nchatgpt_base_url="u"']),
    ('c-multiline-fallback', ['-c', 'model=a\nchatgpt_base_url="u"']), ('c-value-quote', ['-c', 'model=a"b\\c']), ('c-value-emoji', ['-c', 'model=😀']),
    ('c-value-bmp', ['-c', 'model=é中\x7f\t']), ('c-value-control', ['-c', 'model=a\x01b']), ('c-value-cr', ['-c', 'model=a\rb', '-c', 'openai_base_url=a\r\nb']),
    ('c-value-date', ['-c', 'when=2026-11-08', '-c', 'bad=2026-02-30']), ('c-value-numbers', ['-c', 'a=0x10', '-c', 'b=1e5', '-c', 'c=01', '-c', 'd=+inf', '-c', 'e=' + '9' * 4301]),
    ('c-value-array', ['-c', 'a=[1, 2', '-c', 'b=[1, 2]', '-c', 'c=' + '[' * 100 + ']' * 100]), ('c-value-too-deep', ['-c', 'a=' + '[' * 600, '-c', 'b=' + '[' * 600 + ']' * 600]),
    ('c-profiles-too-deep', ['-c', 'profiles.a.' * 2000 + 'x=1', '-c', 'profiles.a.' * 50 + 'chatgpt_base_url=1']),
    ('c-comment', ['-c', 'model="a" # chatgpt_base_url', '-c', '# x=1']), ('c-table-header', ['-c', '[features]\nrespect_system_proxy=true']),
    ('c-key-turkish', ['-c', 'chatgpt_base_urlı=x', '-c', 'CHATGPT_BASE_URL=x']),
    ('enable', ['--enable', 'respect_system_proxy']), ('enable-equals', ['--enable=system_proxy_fallback']), ('enable-other', ['--enable', 'web_search', '--enable=x']),
    ('enable-missing', ['--enable']), ('disable', ['--disable', 'system_proxy_fallback', '--disable=respect_system_proxy']), ('enable-empty', ['--enable=']),
    ('remote', ['--remote', 'ws://example.invalid']), ('remote-equals', ['--remote=ws://example.invalid']), ('remote-missing', ['--remote']),
    ('remote-token', ['--remote-auth-token-env', 'X']), ('remote-token-missing', ['--remote-auth-token-env']), ('oss', ['--oss']), ('oss-equals', ['--oss=1']),
    ('local-provider', ['--local-provider', 'ollama']), ('local-provider-equals', ['--local-provider=ollama']), ('local-provider-missing', ['--local-provider']),
    ('profile-ok', ['--profile', 'deep-review', '-p', '工作 配置', '-preview.v2', '--profile=with spaces', '-p=x']),
    ('profile-escape', ['--profile', '../escape']), ('profile-absolute', ['--profile=/tmp/profile']), ('profile-hidden', ['-p', '.hidden']),
    ('profile-attached-bad', ['-p../x', '-p=/x']), ('profile-empty', ['--profile=', '-p', '']), ('profile-newline', ['-p', 'bad\nname']),
    ('profile-missing', ['-p']), ('profile-long-missing', ['exec', '--profile']), ('profile-nested', ['--profile', 'nested/profile']),
    ('app', ['app']), ('app-after-option', ['-m', 'sample-model', 'app']), ('app-second', ['exec', 'app']), ('app-as-value', ['-m', 'app', 'write a test']),
    ('app-after-dashes', ['--', 'app']), ('app-after-flag', ['--oss', 'app']), ('app-equals', ['app=1']), ('app-twice', ['app', 'app']),
    ('app-after-empty', ['', 'app']), ('app-after-dash', ['-', 'app']),
    ('dashes-stop', ['--', '--remote', 'literal-prompt', '-c', 'chatgpt_base_url=x']), ('value-is-dashes', ['--model', '--', '--config', 'chatgpt_base_url=https://example.invalid']),
    ('value-is-config', ['-m', '-c', 'exec']), ('every-value-option', sum([[o, '--oss'] for o in [
        '-c', '--config', '-p', '--profile', '-C', '--cd', '--enable', '--disable', '-m', '--model', '-s', '--sandbox', '-a', '--ask-for-approval', '-i',
        '--image', '--local-provider', '--remote-auth-token-env', '--remote', '--output-schema', '-o', '--output-last-message']], []) + ['--full-auto', '--oss']),
    ('claude-flags-mean-nothing', ['--settings', BAD_JSON, '--desktop', '--gemini_dir', 'x']),
    ('short-clusters', ['-cfeatures.respect_system_proxy=true', '-Cother', '-pok', '-m=x', '-C']),
    ('unicode-arg', ['中文提示', '-中', 'é=1', '-c', '中=1']),
]


def codex_project(b):
    b.write('project/.codex/config.toml', 'model = "safe-model"\n')
    b.write('project/other/.codex/config.toml', 'chatgpt_base_url = "https://example.invalid"\n')
    b.write('other/.codex/config.toml', 'openai_base_url = "https://example.invalid"\n')
    return {'cwd': 'project'}


for name, args in CODEX_ARGS:
    @case('launch-check/codex-%s' % name, ['launch-check'] + args, tool='codex')
    def _(b):
        return codex_project(b)


for name, args in [('none', []), ('relative', ['-C', 'other']), ('attached', ['-Cother']), ('long', ['--cd', 'other']), ('long-equals', ['--cd=other']),
                   ('short-equals', ['-C=other']), ('up', ['-C', '../other']), ('home', ['--cd', '~/other']), ('home-only', ['-C', '~']),
                   ('unknown-user', ['-C', '~no-such-user-cca-test/x']), ('missing-dir', ['-C', 'nope/deeper']), ('root', ['-C', '/']), ('empty', ['-C', '']),
                   ('twice-last-wins', ['-C', 'a', 'exec', '-C', 'b']), ('twice-long', ['-C', 'a', 'exec', '--cd=b']), ('twice-back', ['-C', 'other', '-C', '.']),
                   ('after-dashes', ['--', '-C', 'other']), ('missing', ['-C']), ('with-profile', ['-C', 'other', '-p', '../x', '--oss'])]:
    @case('launch-check/codex-cd-%s' % name, ['launch-check'] + args, tool='codex')
    def _(b):
        # 第二个 -C 也是相对最初的目录：-C a exec -C b 查的是 project/b，不是 project/a/b
        b.write('project/a/b/.codex/config.toml', 'forced_chatgpt_workspace_id = "w"\n')
        b.write('project/b/.codex/config.toml', 'chatgpt_base_url = "https://example.invalid"\n')
        return codex_project(b)


@case('launch-check/codex-cd-twice-other-way', ['launch-check', '-C', 'a', 'exec', '--cd=b'], tool='codex')
def _(b):
    # 反过来：有问题的在 project/a/b，project/b 没问题——没有可报的
    b.write('project/a/b/.codex/config.toml', 'chatgpt_base_url = "https://example.invalid"\n')
    b.write('project/b/.codex/config.toml', 'model = "safe-model"\n')
    return codex_project(b)


@case('launch-check/codex-cd-absolute', ['launch-check', '-C', '?'], tool='codex')
def _(b):
    patch = codex_project(b)
    return dict(patch, argv=['launch-check', '-C', b.path('other'), '--cd=' + b.path('project/other/../other')])


@case('launch-check/claude-settings-absolute', ['launch-check', '--settings', '?'])
def _(b):
    patch = claude_project(b)
    return dict(patch, argv=['launch-check', '--settings', b.path('project/custom.json'), '--settings=' + b.path('project/sub/../broken.json')])


@case('settings-check/codex-large-environment', ['settings-check'], tool='codex', env={'CCA_POLICY_DEFAULT': '', 'CCA_TEST_LARGE_ENV': 'x' * 90000})
def _(b):
    pass


def codex_tree(b):
    b.write('work/.codex/config.toml', 'features={respect_system_proxy=true}')
    b.write('work/repo/.codex/config.toml', 'profile="../outside"\n[features]\nsystem_proxy_fallback=true\n')   # 项目配置里不许开回退
    b.write('work/repo/pkg/.codex/config.toml', 'model = "unterminated')
    b.write('work/repo/pkg/src/deep/.codex/config.toml', 'chatgpt_base_url="u"')
    b.write('work/repo/other/.codex/config.toml', 'chatgpt_base_url="u"')
    b.write('work/repo/pkg/src/.codex/x.config.toml', 'chatgpt_base_url="u"')     # 项目里的 profile 文件不查
    b.write('work/repo/pkg/src/.claude/settings.json', '{"env":{"NO_PROXY":"x"}}')
    mkdir(b, 'work/repo/pkg/src')


for name, rel in [('in-project', 'work/repo/pkg/src'), ('one-up', 'work/repo/pkg'), ('repo', 'work/repo'), ('top', 'work'), ('sibling', 'work/repo/other'),
                  ('deeper', 'work/repo/pkg/src/deep')]:
    @case('launch-check/codex-project-%s' % name, ['launch-check'], tool='codex')
    def _(b, rel=rel):
        codex_tree(b)
        return {'cwd': rel}


@case('launch-check/codex-project-unreadable', ['launch-check'], tool='codex')
def _(b):
    mkdir(b, 'p/.codex/config.toml')
    link(b, 'p/q/.codex/config.toml', 'nowhere')
    b.write('p/q/r/.codex/config.toml', b'\xff')
    b.write('p/q/r/s/.codex', 'a file')
    return {'cwd': 'p/q/r/s'}


@case('launch-check/codex-home-is-cwd', ['launch-check'], tool='codex')
def _(b):
    toml(b, 'chatgpt_base_url = "u"')


@case('launch-check/codex-home-is-cwd-linked', ['launch-check', '--oss'], tool='codex')
def _(b):
    b.write('dotfiles/codex.toml', 'chatgpt_base_url = "u"\n[features]\nsystem_proxy_fallback = true')
    link(b, '.codex/config.toml', b.path('dotfiles/codex.toml'))
    b.write('.codex/.env', 'OPENAI_API_KEY=dummy')
    return {'cwd': 'sub'}


@case('launch-check/codex-does-not-echo-values', ['launch-check', '-c', 'model=benign', '-c', 'openai_base_url=unique-private-value-2'], tool='codex')
def _(b):
    toml(b, 'chatgpt_base_url="unique-private-value"')
    b.write('.codex/.env', 'OPENAI_API_KEY=unique-private-value-3\n')


# 套到 Python 调用栈的上限前后（RecursionError 也算读不了）。各条路上调用栈已经有几层不一样，上限跟着差一两层：
# 共享的 config.toml 在 launch-check 里比 settings-check 多隔一层（数组 493 层），项目配置一样（494 层），-c 的值浅三层（495 层）
def nested(n, inner=''):
    return 'a = ' + '[' * n + inner + ']' * n


for name, shared, project, args in [
        ('shared-493', nested(493) + '\nopenai_base_url = "u"', None, []), ('shared-494', nested(494), None, []),
        ('project-494', None, nested(494) + '\nopenai_base_url = "u"', []), ('project-495', None, nested(495), []),
        ('c-495', None, None, ['-c', 'a=' + nested(495)[4:], '--oss']), ('c-496', None, None, ['-c', 'a=' + nested(496)[4:]]),
        ('c-profiles-996', None, None, ['-c', 'profiles.a.' * 996 + 'chatgpt_base_url=1']), ('c-profiles-997', None, None, ['-c', 'profiles.a.' * 997 + 'chatgpt_base_url=1']),
        ('shared-profiles-993', 'profiles.a.' * 993 + 'chatgpt_base_url = "u"', None, []), ('shared-profiles-994', 'profiles.a.' * 994 + 'chatgpt_base_url = "u"', None, [])]:
    @case('launch-check/codex-limit-%s' % name, ['launch-check'] + args, tool='codex')
    def _(b, shared=shared, project=project):
        if shared is not None:
            toml(b, shared)
        if project is not None:
            b.write('project/.codex/config.toml', project)
        return {'cwd': 'project'}


@case('launch-check/codex-profile-files-and-flag', ['launch-check', '--profile', 'deep-review', '-c', 'profile="工作 配置"'], tool='codex')
def _(b):
    toml(b, 'profile = "with spaces"\n')
    for n in ('deep-review', '工作 配置', 'review.v2', 'with spaces'):
        toml(b, 'model="sample-model"\n', n + '.config.toml')
    b.write('project/.codex/config.toml', 'profile="../outside"\n')
    return {'cwd': 'project'}


# ---- launch-check：agy 的启动参数 ----------------------------------------------------------------------------------

AGY_ARGS = [
    ('none', []), ('gemini-dir', ['--gemini_dir', '/tmp/x']), ('gemini-dir-dash', ['--gemini-dir=/tmp/x']), ('gemini-dir-equals', ['--gemini_dir=']),
    ('gemini-dir-missing', ['--gemini_dir']), ('gemini-dir-twice', ['--gemini_dir', 'a', '--gemini-dir', 'b', '--gemini_dir=c']),
    ('settings-inline-gcp', ['--settings', '{"gcp": {"project": "p"}}']), ('settings-inline-dotted', ['--settings={"gcp.project": "p", "gcp.location": "l"}']),
    ('settings-inline-ok', ['--settings', '{"theme": "dark"}']), ('settings-inline-space', ['--settings', ' \n{"gcp": 1}']),
    ('settings-inline-invalid', ['--settings', '{"gcp":']), ('settings-inline-too-deep', ['--settings', '{"gcp":' + '[' * 100000]),
    ('settings-not-object', ['--settings', '[]']), ('settings-file', ['--settings', 'session.json']), ('settings-file-equals', ['--settings=./session.json']),
    ('settings-file-clean', ['--settings', 'clean.json']), ('settings-file-list', ['--settings', 'list.json']), ('settings-file-broken', ['--settings', 'broken.json']),
    ('settings-file-missing', ['--settings', 'missing.json']), ('settings-file-directory', ['--settings', '.']), ('settings-empty', ['--settings=']),
    ('settings-file-home', ['--settings', '~/home-agy.json']), ('settings-file-unknown-user', ['--settings', '~no-such-user-cca-test/x.json']),
    ('settings-file-not-utf8', ['--settings', 'binary.json']), ('settings-file-dangling', ['--settings', 'dangling.json']),
    ('settings-missing-value', ['--settings']), ('settings-twice', ['--settings', '{"gcp": 1}', '--settings', 'session.json', '--settings={"gcp": 2}']),
    ('value-is-settings', ['-p', '--settings']), ('value-is-dashes', ['--prompt', '--', '--settings', '{"gcp": 1}']), ('after-dashes', ['--', '--gemini_dir', 'x']),
    ('i-takes-value', ['-i', '--gemini_dir', '--gemini_dir']), ('every-value-option', sum([[o, '--gemini_dir=x'] for o in [
        '--add-dir', '--agent', '--conversation', '--effort', '--input-format', '--json-schema', '--log-file', '--mode', '--model', '--output-format',
        '--print-timeout', '--project', '--settings', '-p', '--print', '--prompt', '-i', '--prompt-interactive', '--gemini_dir', '--gemini-dir']], [])),
    ('other-tools-flags-mean-nothing', ['-c', 'chatgpt_base_url=x', '--desktop', 'app', '--remote', 'x', '-C', 'other']),
]

for name, args in AGY_ARGS:
    @case('launch-check/agy-%s' % name, ['launch-check'] + args, tool='agy')
    def _(b):
        b.write('project/session.json', '{"gcp.location": "l", "gcp": {}}')
        b.write('project/clean.json', '{"theme": "light"}')
        b.write('project/list.json', '["gcp"]')
        b.write('project/broken.json', '{"gcp"')
        b.write('project/binary.json', b'{"gcp": "\xff"}')
        link(b, 'project/dangling.json', 'nowhere')
        b.write('home-agy.json', '{"gcp.project": "p"}')
        # 项目目录里别的工具的设置、agy 自己的 .agents：都不查
        b.write('project/.claude/settings.json', '{"env":{"NO_PROXY":"x"}}')
        b.write('project/.codex/config.toml', 'chatgpt_base_url = "u"')
        b.write('project/.agents/settings.json', '{"gcp": 1}')
        return {'cwd': 'project'}


@case('launch-check/agy-shared-and-args', ['launch-check', '--gemini_dir', 'x', '--settings', '{"gcp": 1}', '--model'], tool='agy')
def _(b):
    agy(b, {'gcp.project': 'p'})


# ---- settings-explain：Claude 的说明；别的工具照同样的规则说（bash 不给它们用，子命令还是在） ------------------------

EXPLAIN = [
    ('none', {}, None), ('clean', {'env': {'EDITOR': 'vi'}}, {}),
    ('api', {'env': {'ANTHROPIC_BASE_URL': 'https://api.example.com/anthropic', 'ANTHROPIC_AUTH_TOKEN': 'sk-fix-0000', 'ANTHROPIC_MODEL': 'k2'}, 'apiKeyHelper': 'echo sk-fix-0000'}, None),
    ('proxy', {'env': {'NO_PROXY': '*'}}, None), ('helper', {'apiKeyHelper': 'echo x'}, None),
    ('cloud', {'env': {'CLAUDE_CODE_USE_BEDROCK': '1', 'claude_code_provider_managed_by_host': '1'}}, None),
    ('login', {'env': {'CLAUDE_CONFIG_DIR': '/x', 'CLAUDE_CODE_OAUTH_TOKEN': 'dummy', 'CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR': '3', 'CLAUDE_SECURESTORAGE_CONFIG_DIR': '/y'}}, None),
    ('every-kind', {'env': {'CLAUDE_CONFIG_DIR': '/x', 'https_proxy': 'x', 'CLAUDE_CODE_USE_VERTEX': '1', 'ANTHROPIC_API_KEY': 'dummy', 'ALL_PROXY': 'y'}, 'apiKeyHelper': 'x'}, None),
    ('local-only', None, {'env': {'HTTP_PROXY': 'x'}}),
    ('both', {'env': {'HTTPS_PROXY': 'x'}}, {'env': {'CLAUDE_CODE_USE_GATEWAY': '1', 'HTTPS_PROXY': 'y'}}),
    ('both-api', {'env': {'CLAUDE_CODE_USE_FOUNDRY': '1'}}, {'apiKeyHelper': 'x', 'env': {'NO_PROXY': 'x'}}),
    ('one-broken', '{oops', {'env': {'HTTPS_PROXY': 'x'}}), ('both-broken', '[]', '{"env": 1}'), ('env-broken', '{"env": []}', None),
    ('odd-names', {'env': {'HTTPS_PROXY\n': 'x', 'ANTHROPıC_BASE_URL': 'y', 'HTTPſ_PROXY': 'z', 'APIKEYHELPER': 'no', 'apiKeyHelper': 'no'}}, None),
]

for name, shared, local in EXPLAIN:
    for tool in TOOLS if name in ('none', 'api') else ('claude',):
        @case('settings-explain/%s-shared-%s' % (tool, name), ['settings-explain', 'shared'], tool=tool)
        def _(b, shared=shared, local=local):
            for data, n in ((shared, 'settings.json'), (local, 'settings.local.json')):
                if isinstance(data, str):
                    b.write('.claude/' + n, data)
                elif data is not None:
                    settings(b, data, n)
            toml(b, 'chatgpt_base_url = "u"\n[profiles.p]\nmodel_provider = "x"')
            b.write('.codex/.env', 'CODEX_HOME=/x')
            agy(b, {'gcp': 1, 'gcp.project': 'p'})


@case('settings-explain/claude-mode-is-anything-else', ['settings-explain', 'whatever', '--desktop'])
def _(b):
    settings(b, {'env': {'NO_PROXY': 'x'}})


@case('settings-explain/no-mode', ['settings-explain'])
def _(b):
    pass


@case('settings-explain/claude-launch-everything', ['settings-explain', 'launch', '--desktop', '--settings', BAD_JSON, '--settings', 'custom.json', '--settings',
                                                    'broken.json', '--settings', '{"env":{"NO_PROXY":"x","CLAUDE_CODE_USE_MANTLE":"1"}}', '--settings', '{', '--model'])
def _(b):
    settings(b, {'env': {'HTTPS_PROXY': 'x'}})
    settings(b, {'apiKeyHelper': 'x'}, 'settings.local.json')
    b.write('project/.claude/settings.json', '{"env":{"ANTHROPIC_API_KEY":"dummy"}}')
    b.write('.claude/x', 'x')
    return claude_project(b)


@case('settings-explain/claude-launch-outside-home', ['settings-explain', 'launch', '--settings', '/etc/hosts', '--settings', '/nonexistent/x.json'])
def _(b):
    pass


for tool, args in [('codex', ['-c', 'chatgpt_base_url=x', '--remote', 'x', '--oss', 'app', '-p', '../x', '--enable', 'respect_system_proxy', '-c', 'nope', '--cd']),
                   ('agy', ['--gemini_dir', 'x', '--settings', '{"gcp": 1}', '--settings', 'missing.json', '--model'])]:
    @case('settings-explain/%s-launch' % tool, ['settings-explain', 'launch'] + args, tool=tool)
    def _(b):
        toml(b, 'chatgpt_base_url = "u"\nprofile = 1')
        toml(b, 'x = ', 'p.config.toml')
        b.write('.codex/.env', 'CODEX_HOME=/x')
        b.write('project/.codex/config.toml', 'openai_base_url = "u"\n[features]\nrespect_system_proxy = true')
        agy(b, {'gcp': 1})
        return {'cwd': 'project'}


# ---- settings-fix plan / apply ------------------------------------------------------------------------------------

FIX_API = {'env': {'ANTHROPIC_BASE_URL': 'https://api.example.com/anthropic', 'ANTHROPIC_AUTH_TOKEN': 'sk-fix-0000', 'ANTHROPIC_MODEL': 'kimi-k2',
                   'ANTHROPIC_DEFAULT_OPUS_MODEL': 'claude-opus-4-5', 'API_TIMEOUT_MS': '600000'},
           'model': 'kimi-k2', 'apiKeyHelper': 'echo sk-fix-0000', 'permissions': {'allow': ['Bash(ls:*)']}}

FIXES = [
    ('none', None, None), ('clean', {'model': 'opus', 'env': {'EDITOR': 'vi', 'ANTHROPIC_MODEL': 'not-claude', 'ANTHROPIC_CUSTOM_HEADERS': 'x'}}, {}),
    ('api', FIX_API, None), ('proxy-only', {'env': {'HTTPS_PROXY': 'http://127.0.0.1:9', 'ANTHROPIC_MODEL': 'kimi-k2', 'ANTHROPIC_CUSTOM_HEADERS': 'x'}, 'model': 'kimi-k2'}, None),
    ('login-only', {'env': {'CLAUDE_CONFIG_DIR': '/x', 'CLAUDE_CODE_SUBAGENT_MODEL': 'k2'}, 'model': 'k2'}, None),
    ('cloud', {'env': {'CLAUDE_CODE_USE_BEDROCK': '1', 'ANTHROPIC_MODEL': 'us.anthropic.claude-x', 'ANTHROPIC_SMALL_FAST_MODEL': 'haiku', 'EDITOR': 'vi',
                       'ANTHROPIC_CUSTOM_HEADERS': 'x: y', 'CLAUDE_CODE_SUBAGENT_MODEL': 'custom'}, 'model': 'opus[1m]'}, None),
    ('helper-only', {'apiKeyHelper': 'echo x', 'model': 123, 'env': {'ANTHROPIC_DEFAULT_SONNET_MODEL': None, 'ANTHROPIC_DEFAULT__MODEL': 'x', 'ANTHROPIC_DEFAULT_MODEL': 'y'}}, None),
    ('helper-no-env', {'apiKeyHelper': ['x'], 'model': ' Sonnet '}, None), ('env-emptied', {'a': 1, 'env': {'NO_PROXY': 'x', 'all_proxy': 'y'}, 'z': 2}, None),
    ('models', {'env': {'ANTHROPIC_API_KEY': 'dummy', 'ANTHROPIC_MODEL': 'CLAUDE-opus-4-5', 'ANTHROPIC_SMALL_FAST_MODEL': 'best', 'ANTHROPIC_DEFAULT_OPUS_MODEL': 'opusplan',
                        'ANTHROPIC_DEFAULT_HAIKU_MODEL': 'fable[1M]', 'ANTHROPIC_DEFAULT_SONNET_MODEL': 'claude-', 'ANTHROPIC_DEFAULT_X_MODEL': 'default\n',
                        'ANTHROPIC_DEFAULT_Y_MODEL': 'gpt', 'anthropic_model': 'gpt', 'ANTHROPIC_MODEL\n': 'gpt', 'ANTHROPIC_DEFAULT_Z9_MODEL': 'haiKu',
                        'ANTHROPIC_DEFAULT_I_MODEL': 'haıku', 'CLAUDE_CODE_SUBAGENT_MODEL': ['opus'], 'ANTHROPIC_MODEL_X': 'gpt'}, 'model': 'claude-sonnet-4-5[1m]'}, None),
    ('both', {'env': {'HTTPS_PROXY': 'x'}, 'k': 1}, {'apiKeyHelper': 'x', 'env': {'ANTHROPIC_CUSTOM_HEADERS': 'h', 'OK': '1'}, 'model': 'other'}),
    ('local-only', {'env': {'EDITOR': 'vi'}}, {'env': {'ANTHROPIC_BASE_URL': 'u'}}),
    ('first-clean-second-bad-json', {'env': {}}, '{"env": {'), ('first-bad-second-broken', {'env': {'NO_PROXY': 'x'}}, '[]'),
    ('bad-json', '{"env": {', None), ('not-object', '"x"', None), ('env-not-object', '{"env": [], "apiKeyHelper": "x"}', None), ('env-null', '{"env": null}', None),
    ('empty-file', '', None), ('bom', '﻿{"env": {"NO_PROXY": "x"}}', None), ('not-utf8', b'{"env": {"NO_PROXY": "\xff"}}', None),
    ('too-deep', '{"env": {"NO_PROXY": "x"}, "x": ' + '[' * 300000 + ']' * 300000 + '}', None), ('int-too-long', '{"env": {"NO_PROXY": "x"}, "n": ' + '9' * 4301 + '}', None),
    ('odd-values', '{"env": {"NO_PROXY": "x", "E": 1e400, "F": -0.0, "G": 1E5, "H": 12345678901234567890123, "I": "\\u00e9\\ud83d\\ude00\\u007f", "J": [],'
                   ' "K": {}, "L": 1.0, "M": 0.1}, "a": NaN, "dup": 1, "dup": 2, "apiKeyHelper": 0}', None),
    ('crlf-and-indent', '{\r\n\t"env": {\r\n\t\t"NO_PROXY": "x",\r\n\t\t"KEEP": "中文"\r\n\t},\r\n\t"list": [1, 2]\r\n}\r\n', None),
    ('turkish-i', {'env': {'ANTHROPıC_BASE_URL': 'u', 'ANTHROPIC_MODEL': 'gpt', 'HTTPſ_PROXY': 'x'}}, None),
]


def fix_fixture(b, shared, local):
    for data, n in ((shared, 'settings.json'), (local, 'settings.local.json')):
        if isinstance(data, (str, bytes)):
            b.write('.claude/' + n, data)
        elif data is not None:
            settings(b, data, n, indent=2)


def fix_digest(b, names=('settings.json', 'settings.local.json'), drops=None, home=''):
    """照 settings_fix_digest 算这次要改的摘要（drops：[(去掉 env 里的, 去掉顶层的)…]，和要改的文件一一对应）"""
    h = hashlib.sha256()
    for n, (drop_env, drop_top) in zip(names, drops):
        p = b.path(home + '.claude/' + n)
        with open(p, 'rb') as f:
            raw = f.read()
        h.update(json.dumps([p, hashlib.sha256(raw).hexdigest(), drop_env, drop_top]).encode())
    return h.hexdigest()[:16]


def same_second():
    """备份的文件名里有这一秒的时间，run.py 不换文件名：这一秒剩下不到一半时等到下一秒开头，两边才落在同一秒里。
    返回这一秒（子命令是在 TZ=Asia/Shanghai 下跑的）"""
    left = 1 - time.time() % 1
    if left < 0.55:
        time.sleep(left + 0.01)
    return datetime.datetime.fromtimestamp(time.time(), datetime.timezone(datetime.timedelta(hours=8)))


for name, shared, local in FIXES:
    for tool in TOOLS if name in ('api', 'none', 'bad-json') else ('claude',):
        @case('settings-fix/plan-%s-%s' % (name, tool), ['settings-fix', 'plan'], tool=tool)
        def _(b, shared=shared, local=local):
            fix_fixture(b, shared, local)


@case('settings-fix/plan-is-directory', ['settings-fix', 'plan'])
def _(b):
    mkdir(b, '.claude/settings.json')
    settings(b, {'env': {'NO_PROXY': 'x'}}, 'settings.local.json')


@case('settings-fix/plan-dangling-link', ['settings-fix', 'plan'])
def _(b):
    settings(b, {'env': {'NO_PROXY': 'x'}})
    link(b, '.claude/settings.local.json', 'nowhere.json')


@case('settings-fix/plan-link-loop', ['settings-fix', 'plan'])
def _(b):
    link(b, '.claude/settings.json', 'settings.json')


@case('settings-fix/plan-linked', ['settings-fix', 'plan'])
def _(b):
    b.json('dotfiles/settings.json', {'env': {'HTTPS_PROXY': 'http://127.0.0.1:9'}})
    link(b, '.claude/settings.json', b.path('dotfiles/settings.json'))


@case('settings-fix/plan-master-is-file', ['settings-fix', 'plan'])
def _(b):
    b.write('.claude', 'x')


@case('settings-fix/plan-ignores-project-and-args', ['settings-fix', 'plan', ''])
def _(b):
    b.write('project/.claude/settings.json', '{"env":{"ANTHROPIC_BASE_URL":"u"}}')
    return {'cwd': 'project'}


@case('settings-fix/no-action', ['settings-fix'])
def _(b):
    pass


@case('settings-fix/too-many-arguments', ['settings-fix', 'apply', 'x', 'y'])
def _(b):
    pass


# apply：先照同一套规则算出摘要（参数里要带）
APPLIES = [
    ('api', FIX_API, None, [(['ANTHROPIC_BASE_URL', 'ANTHROPIC_AUTH_TOKEN', 'ANTHROPIC_MODEL'], ['apiKeyHelper', 'model'])]),
    ('proxy-only', {'env': {'HTTPS_PROXY': 'http://127.0.0.1:9', 'ANTHROPIC_MODEL': 'kimi-k2'}, 'model': 'kimi-k2'}, None, [(['HTTPS_PROXY'], [])]),
    ('env-emptied', {'a': 1, 'env': {'NO_PROXY': 'x', 'all_proxy': 'y'}, 'z': 2}, None, [(['NO_PROXY', 'all_proxy'], [])]),
    ('both', {'env': {'HTTPS_PROXY': 'x'}, 'k': 1}, {'apiKeyHelper': 'x', 'env': {'ANTHROPIC_CUSTOM_HEADERS': 'h', 'OK': '1'}, 'model': 'other'},
     [(['HTTPS_PROXY'], []), (['ANTHROPIC_CUSTOM_HEADERS'], ['apiKeyHelper', 'model'])]),
    ('local-only', {'env': {'EDITOR': 'vi'}}, {'env': {'ANTHROPIC_BASE_URL': 'u'}}, [(['ANTHROPIC_BASE_URL'], [])]),
    ('odd-values', FIXES[-3][1], None, [(['NO_PROXY'], [])]), ('crlf-and-indent', FIXES[-2][1], None, [(['NO_PROXY'], [])]),
    ('turkish-i', FIXES[-1][1], None, [(['ANTHROPıC_BASE_URL', 'HTTPſ_PROXY', 'ANTHROPIC_MODEL'], [])]),
]

for name, shared, local, drops in APPLIES:
    for tool in TOOLS if name == 'api' else ('claude',):
        @case('settings-fix/apply-%s-%s' % (name, tool), ['settings-fix', 'apply', '?'], tool=tool, mask=STAMP)
        def _(b, shared=shared, local=local, drops=drops, name=name):
            fix_fixture(b, shared, local)
            names = ['settings.local.json'] if name == 'local-only' else ['settings.json', 'settings.local.json']
            same_second()
            return {'argv': ['settings-fix', 'apply', fix_digest(b, names, drops)]}


@case('settings-fix/apply-keeps-mode', ['settings-fix', 'apply', '?'], mask=STAMP)
def _(b):
    settings(b, {'env': {'NO_PROXY': 'x'}, 'k': 1}, mode=0o644, indent=2)
    settings(b, {'apiKeyHelper': 'x'}, 'settings.local.json', mode=0o640)
    same_second()
    return {'argv': ['settings-fix', 'apply', fix_digest(b, drops=[(['NO_PROXY'], []), ([], ['apiKeyHelper'])])]}


@case('settings-fix/apply-linked-to-dotfiles', ['settings-fix', 'apply', '?'], mask=STAMP)
def _(b):
    # 链接留着，改它指向的文件；备份在 ~/.claude 里，不写进链接指向的目录
    b.json('dotfiles/settings.json', {'env': {'HTTPS_PROXY': 'http://127.0.0.1:9', 'KEEP': '1'}}, 0o644)
    link(b, '.claude/settings.json', b.path('dotfiles/settings.json'))
    b.json('dotfiles/local.json', {'apiKeyHelper': 'x'})
    link(b, '.claude/settings.local.json', '../dotfiles/local.json')
    same_second()
    return {'argv': ['settings-fix', 'apply', fix_digest(b, drops=[(['HTTPS_PROXY'], []), ([], ['apiKeyHelper'])])]}


@case('settings-fix/apply-linked-proxy-only', ['settings-fix', 'apply', '?'], mask=STAMP)
def _(b):
    # 回归 10c 的那一幕：只有代理，改完不提 cca add api
    b.json('dotfiles/settings.json', {'permissions': {'allow': ['Bash(ls:*)']}, 'env': {'HTTPS_PROXY': 'http://127.0.0.1:9'}}, indent=None)
    link(b, '.claude/settings.json', b.path('dotfiles/settings.json'))
    same_second()
    return {'argv': ['settings-fix', 'apply', fix_digest(b, ['settings.json'], [(['HTTPS_PROXY'], [])])]}


@case('settings-fix/apply-stale-digest', ['settings-fix', 'apply', '0000000000000000'])
def _(b):
    fix_fixture(b, FIX_API, None)


@case('settings-fix/apply-no-digest', ['settings-fix', 'apply'])
def _(b):
    fix_fixture(b, FIX_API, None)


@case('settings-fix/apply-nothing-to-fix', ['settings-fix', 'apply', ''])
def _(b):
    settings(b, {'env': {'EDITOR': 'vi'}})


@case('settings-fix/apply-nothing-at-all', ['settings-fix', 'apply', 'e3b0c44298fc1c14'])
def _(b):
    pass


@case('settings-fix/apply-bad-json', ['settings-fix', 'apply', 'x'])
def _(b):
    fix_fixture(b, {'env': {'NO_PROXY': 'x'}}, '{"env": {')


@case('settings-fix/apply-is-directory', ['settings-fix', 'apply', 'x'])
def _(b):
    mkdir(b, '.claude/settings.json')


@case('settings-fix/apply-dangling-link', ['settings-fix', 'apply', 'x'])
def _(b):
    link(b, '.claude/settings.json', 'nowhere.json')


@case('settings-fix/any-other-action-applies', ['settings-fix', 'check', '?'], mask=STAMP)
def _(b):
    fix_fixture(b, {'env': {'NO_PROXY': 'x'}}, None)
    same_second()
    return {'argv': ['settings-fix', 'check', fix_digest(b, drops=[(['NO_PROXY'], [])])]}


@case('settings-fix/apply-readonly-flag-is-ignored', ['settings-fix', 'apply', '?'], mask=STAMP, env={'CCA_RO': '1'})
def _(b):
    fix_fixture(b, {'env': {'NO_PROXY': 'x'}}, None)
    same_second()
    return {'argv': ['settings-fix', 'apply', fix_digest(b, drops=[(['NO_PROXY'], [])])]}


@case('settings-fix/apply-backup-name-taken', ['settings-fix', 'apply', '?'], mask=STAMP)
def _(b):
    # 这一秒（和下一秒）的备份名已经有了：后面加 -1；-1 也有了的那份加 -2
    fix_fixture(b, {'env': {'NO_PROXY': 'x'}}, {'apiKeyHelper': 'x'})
    digest = fix_digest(b, drops=[(['NO_PROXY'], []), ([], ['apiKeyHelper'])])
    now = same_second()
    for i in range(3):
        stamp = (now + datetime.timedelta(seconds=i)).strftime('%Y%m%d-%H%M%S')
        b.write('.claude/settings.json.cca-backup-' + stamp, 'older backup')
        b.write('.claude/settings.local.json.cca-backup-' + stamp, 'older backup')
        b.write('.claude/settings.local.json.cca-backup-%s-1' % stamp, 'older backup')
        link(b, '.claude/settings.local.json.cca-backup-%s-2' % stamp, 'nowhere')     # 是个坏链接也算有了
    return {'argv': ['settings-fix', 'apply', digest]}


@case('settings-fix/apply-backup-names-all-taken', ['settings-fix', 'apply', '?'], mask=STAMP)
def _(b):
    # 一百个名字都有了：改不了，什么都不动
    fix_fixture(b, {'env': {'NO_PROXY': 'x'}}, None)
    digest = fix_digest(b, drops=[(['NO_PROXY'], [])])
    now = same_second()
    for i in range(3):
        stamp = (now + datetime.timedelta(seconds=i)).strftime('%Y%m%d-%H%M%S')
        for n in range(100):
            b.write('.claude/settings.json.cca-backup-%s%s' % (stamp, '-%d' % n if n else ''), '')
    return {'argv': ['settings-fix', 'apply', digest]}


LONG_NAME = 'x' * 250   # 文件名本身放得下，save_json 旁边那个临时文件（后面加 .cca-进程号.tmp）的名字就超长了


@case('settings-fix/apply-cannot-write', ['settings-fix', 'apply', '?'], mask=STAMP)
def _(b):
    b.json('dotfiles/' + LONG_NAME, {'env': {'NO_PROXY': 'x'}})
    link(b, '.claude/settings.json', b.path('dotfiles/' + LONG_NAME))
    same_second()
    return {'argv': ['settings-fix', 'apply', fix_digest(b, drops=[(['NO_PROXY'], [])])]}


@case('settings-fix/apply-second-cannot-write', ['settings-fix', 'apply', '?'], mask=STAMP)
def _(b):
    settings(b, {'env': {'ANTHROPIC_API_KEY': 'dummy'}}, indent=2)
    b.json('dotfiles/' + LONG_NAME, {'apiKeyHelper': 'x'})
    link(b, '.claude/settings.local.json', b.path('dotfiles/' + LONG_NAME))
    same_second()
    return {'argv': ['settings-fix', 'apply', fix_digest(b, drops=[(['ANTHROPIC_API_KEY'], []), ([], ['apiKeyHelper'])])]}


CHANGING = '{"env": {"NO_PROXY": "x"}}'


@case('settings-fix/apply-changed-while-backing-up', ['settings-fix', 'apply', '?'], stdin=CHANGING, mask=STAMP)
def _(b):
    # settings.json 指向标准输入：第一遍读到这段 JSON，备份完再读一遍已经读到头了——和“备份的这会儿又被改过”一样，都不动
    link(b, '.claude/settings.json', '/dev/stdin')
    h = hashlib.sha256(json.dumps([b.path('.claude/settings.json'), hashlib.sha256(CHANGING.encode()).hexdigest(), ['NO_PROXY'], []]).encode())
    same_second()
    return {'argv': ['settings-fix', 'apply', h.hexdigest()[:16]]}


# ---- codex-fallback-off -------------------------------------------------------------------------------------------

for name, text in [('none', None), ('inline-off', 'features={system_proxy_fallback=false}'), ('table-off', '[features]\nsystem_proxy_fallback=false'),
                   ('on', 'features={system_proxy_fallback=true}'), ('string', 'features={system_proxy_fallback="false"}'), ('zero', 'features.system_proxy_fallback = 0'),
                   ('invalid', 'features={'), ('missing-key', '[features]\nother = false'), ('no-features', 'model = "m"'), ('features-not-table', 'features = false'),
                   ('off-but-other-problem', 'chatgpt_base_url = "u"\n[features]\nsystem_proxy_fallback=false'),
                   ('off-but-profile-on', 'profile="x"\n[features]\nsystem_proxy_fallback=false\n[profiles.x.features]\nsystem_proxy_fallback=true\n'),
                   ('off-and-respect-off', '[features]\nsystem_proxy_fallback=false\nrespect_system_proxy=false'), ('date', 'features.system_proxy_fallback = 2026-11-08'),
                   ('too-deep', 'features.system_proxy_fallback = false\na = ' + '[' * 600 + ']' * 600), ('empty', '')]:
    for tool in TOOLS if name in ('table-off', 'on') else ('codex',):
        @case('codex-fallback-off/%s-%s' % (name, tool), ['codex-fallback-off'], tool=tool)
        def _(b, text=text):
            if text is not None:
                toml(b, text)


for n in (494, 495, 496):
    # 第一遍读 config.toml 时调用栈比接着查共享设置的那一遍浅两层：数组里放字符串时前一遍到 495 层，后一遍到 494 层
    @case('codex-fallback-off/limit-%d' % n, ['codex-fallback-off'], tool='codex')
    def _(b, n=n):
        toml(b, '[features]\nsystem_proxy_fallback=false\n[x]\n' + nested(n, '"x"'))


@case('codex-fallback-off/off-but-profile-file-on', ['codex-fallback-off'], tool='codex')
def _(b):
    toml(b, '[features]\nsystem_proxy_fallback=false\n')
    toml(b, 'features={system_proxy_fallback=true}\n', 'deep.config.toml')


@case('codex-fallback-off/off-but-env-bad', ['codex-fallback-off'], tool='codex')
def _(b):
    toml(b, '[features]\nsystem_proxy_fallback=false\n')
    b.write('.codex/.env', 'OPENAI_API_KEY=dummy\n')


@case('codex-fallback-off/off-with-clean-profile-and-env', ['codex-fallback-off'], tool='codex')
def _(b):
    toml(b, '[features]\nsystem_proxy_fallback=false\n')
    toml(b, 'model = "m"\n[features]\nsystem_proxy_fallback=false\n', 'p.config.toml')
    b.write('.codex/.env', 'CODEX_CA_CERTIFICATE=/x\n')


@case('codex-fallback-off/unreadable', ['codex-fallback-off'], tool='codex')
def _(b):
    mkdir(b, '.codex/config.toml')


@case('codex-fallback-off/dangling-link', ['codex-fallback-off'], tool='codex')
def _(b):
    link(b, '.codex/config.toml', 'nowhere')


@case('codex-fallback-off/not-utf8', ['codex-fallback-off'], tool='codex')
def _(b):
    b.write('.codex/config.toml', b'[features]\nsystem_proxy_fallback=false\n# \xff')


@case('codex-fallback-off/extra-argument', ['codex-fallback-off', 'x'], tool='codex')
def _(b):
    toml(b, '[features]\nsystem_proxy_fallback=false\n')


# ---- 主目录的路径不寻常：带空格、引号、中文，或者 HOME 末尾带斜杠 ----------------------------------------------------

ODD_HOME = "my home's 目录"


def odd_home(b, home=ODD_HOME):
    b.json(home + '/.claude/settings.json', {'env': {'NO_PROXY': 'x', 'ANTHROPIC_BASE_URL': 'u'}, 'k': 1})
    b.write(home + '/.codex/config.toml', 'chatgpt_base_url = "u"')
    b.write(home + '/.codex/a b.config.toml', 'openai_base_url = "u"')
    b.json(home + '/.gemini/antigravity-cli/settings.json', {'gcp': 1})
    b.write(home + '/proj/.claude/settings.local.json', '{"apiKeyHelper":"x"}')
    b.write(home + '/proj/.codex/config.toml', 'model_provider = "x"')
    b.write(home + '/proj/s.json', '{"env":{"ALL_PROXY":"x"}}')
    b.write(home + '/other/.codex/config.toml', 'openai_base_url = "u"')
    return {'home': home, 'cwd': home.rstrip('/') + '/proj'}


for home, tag in [(ODD_HOME, 'odd'), ('plain/', 'trailing-slash'), ('plain//', 'trailing-slashes'), ('a/../b/./c', 'not-normalized')]:
    for tool, args in [('claude', ['--settings', 's.json', '--settings=~/proj/s.json']), ('codex', ['-C', '~/other', '-p', 'a b']), ('agy', ['--settings', '~/proj/s.json'])]:
        if tag == 'odd':
            @case('home-%s/settings-check-%s' % (tag, tool), ['settings-check'], tool=tool)
            def _(b, home=home):
                return odd_home(b, home)

        if tag == 'odd' or tool != 'agy':
            @case('home-%s/launch-check-%s' % (tag, tool), ['launch-check'] + args, tool=tool)
            def _(b, home=home):
                return odd_home(b, home)

        if tag == 'odd' or tool == 'claude':
            @case('home-%s/settings-explain-%s' % (tag, tool), ['settings-explain', 'launch'] + args, tool=tool)
            def _(b, home=home):
                return odd_home(b, home)

    if tag == 'odd':
        @case('home-%s/settings-fix-plan' % tag, ['settings-fix', 'plan'])
        def _(b, home=home):
            return odd_home(b, home)

    @case('home-%s/settings-fix-apply' % tag, ['settings-fix', 'apply', '?'], mask=STAMP)
    def _(b, home=home):
        patch = odd_home(b, home)
        same_second()
        # 摘要里的路径是 HOME 去掉末尾的斜杠后直接拼出来的（不整理 .. 这些）
        digest = fix_digest(b, ['settings.json'], [(['NO_PROXY', 'ANTHROPIC_BASE_URL'], [])], home.rstrip('/') + '/')
        return dict(patch, argv=['settings-fix', 'apply', digest])

    if tag == 'odd':
        @case('home-%s/codex-fallback-off' % tag, ['codex-fallback-off'], tool='codex')
        def _(b, home=home):
            b.write(home + '/.codex/config.toml', '[features]\nsystem_proxy_fallback = false\n')
            return {'home': home}
