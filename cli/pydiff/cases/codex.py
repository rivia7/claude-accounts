"""Codex 的会话库里经账号目录的路径改写到 ~/.codex（cli/src/cmd/codex.rs 的 codex_fix_rollouts；Codex 的 `link` 最后一步）。

原来用 Python 自带的 sqlite3 库，cca-core 起系统的 sqlite3 命令：两边改出来的库字节不一样（文件头里记着 SQLite 的版本），
run.py 对 .sqlite 文件比的是 `.dump` 的内容。夹具里的库在用例的 setup 里用 Python 的 sqlite3 建。"""
import os
import sqlite3

from run import case
from cases.router import ident
from cases.table import world

ERIN = 'erin@example.com'
AID = ident(ERIN)
# link 第一次给账号定设备标识，是随机的：遮掉
IDS = [(r'[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}', '<随机的 UUID>'), (r'\b[0-9a-f]{64}\b', '<随机的标识>')]


def acct(b, rest=''):
    return b.path('.cca/codex/accounts/%s/codex/%s' % (AID, rest))


def db(b, rel, rows, schema='CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL, title TEXT)'):
    p = b.path(rel)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    con = sqlite3.connect(p)
    con.execute(schema)
    if rows:
        con.executemany('INSERT INTO threads VALUES (%s)' % ', '.join('?' * len(rows[0])), rows)
    con.commit()
    con.close()
    return p


def sessions(b, *names, root='.codex'):
    for n in names:
        b.write('%s/%s' % (root, n), '{}\n', 0o644)


def codex_world(b, toml=None):
    world(b, codex={ERIN: 'HK-A'})
    if toml is not None:
        b.write('.codex/config.toml', toml, 0o644)


def rows(b):
    """一批会话：有对应文件的、没有的、归档的、名字里有引号和逗号的、只有带后缀的文件的、不经账号目录的、别的账号目录的"""
    return [
        ('t-moved', acct(b, 'sessions/2026/11/03/rollout-a.jsonl'), 'a'),
        ('t-no-file', acct(b, 'sessions/2026/11/03/rollout-gone.jsonl'), 'b'),
        ('t-archived', acct(b, 'archived_sessions/rollout-old.jsonl'), 'c'),
        ("t-it's, quoted", acct(b, "sessions/2026/11/03/rollout-it's, here.jsonl"), 'd'),
        ('t-prefix', acct(b, 'sessions/2026/11/03/rollout-b.jsonl'), 'e'),
        ('t-elsewhere', b.path('.codex/sessions/2026/11/03/rollout-a.jsonl'), 'f'),
        ('t-other-account', b.path('.cca/codex/accounts/gone-000000/codex/sessions/2026/11/03/rollout-a.jsonl'), 'g'),
        ('t-not-sessions', acct(b, 'worktrees/x/rollout-a.jsonl'), 'h'),
        ('t-directory', acct(b, 'sessions/'), 'i'),
    ]


FILES = ['sessions/2026/11/03/rollout-a.jsonl', 'archived_sessions/rollout-old.jsonl', "sessions/2026/11/03/rollout-it's, here.jsonl",
         'sessions/2026/11/03/rollout-b.jsonl.bak']


@case('codex-rollouts/rewrites', ['link', ERIN], tool='codex', mask=IDS)
def _(b):
    codex_world(b)
    sessions(b, *FILES)
    db(b, '.codex/state_5.sqlite', rows(b))


@case('codex-rollouts/nothing-to-fix', ['link', ERIN], tool='codex', mask=IDS)
def _(b):
    codex_world(b)
    sessions(b, *FILES)
    db(b, '.codex/state_5.sqlite', [r for r in rows(b) if r[0] in ('t-elsewhere', 't-no-file')])


@case('codex-rollouts/several-databases', ['link', ERIN], tool='codex', mask=IDS)
def _(b):
    codex_world(b)
    sessions(b, *FILES)
    db(b, '.codex/state_5.sqlite', rows(b)[:2])
    db(b, '.codex/state_12.sqlite', rows(b)[2:3])
    db(b, '.codex/state_7.sqlite', [('x', 1)], schema='CREATE TABLE threads (id TEXT, title INTEGER)')          # 没有 rollout_path 这一列
    db(b, '.codex/state_8.sqlite', [], schema='CREATE TABLE other (id TEXT)')                                    # 没有 threads 表
    b.write('.codex/state_9.sqlite', 'this is not a database, just text that is long enough to not look empty\n', 0o644)
    b.write('.codex/state_.sqlite', '', 0o644)
    db(b, '.codex/other_5.sqlite', rows(b)[:1])                                                                  # 名字对不上：不看
    db(b, '.codex/state_5.sqlite.bak', rows(b)[:1])


@case('codex-rollouts/integer-ids', ['link', ERIN], tool='codex', mask=IDS)
def _(b):
    codex_world(b)
    sessions(b, *FILES)
    db(b, '.codex/state_5.sqlite', [(i + 1, r[1]) for i, r in enumerate(rows(b))], schema='CREATE TABLE threads (id INTEGER PRIMARY KEY, rollout_path TEXT)')


@case('codex-rollouts/null-and-blob-paths', ['link', ERIN], tool='codex', mask=IDS)
def _(b):
    codex_world(b)
    sessions(b, *FILES)
    db(b, '.codex/state_5.sqlite', [('t-null', None), ('t-blob', b'\x00\x01'), ('t-int', 5), ('t-moved', acct(b, 'sessions/2026/11/03/rollout-a.jsonl'))],
       schema='CREATE TABLE threads (id TEXT, rollout_path)')


@case('codex-rollouts/sqlite-home-elsewhere', ['link', ERIN], tool='codex', mask=IDS)
def _(b):
    codex_world(b, 'model = "x"\nsqlite_home = """\n%s"""\n' % b.path('databases'))
    sessions(b, *FILES)
    db(b, 'databases/state_5.sqlite', rows(b))
    db(b, '.codex/state_5.sqlite', rows(b))          # 设了 sqlite_home 就不看 ~/.codex 里的


@case('codex-rollouts/sqlite-home-tilde', ['link', ERIN], tool='codex', mask=IDS)
def _(b):
    codex_world(b, 'sqlite_home = "~/databases"\n')
    sessions(b, *FILES)
    db(b, 'databases/state_5.sqlite', rows(b))


@case('codex-rollouts/sqlite-home-empty', ['link', ERIN], tool='codex', mask=IDS)
def _(b):
    codex_world(b, 'sqlite_home = ""\n')
    sessions(b, *FILES)
    db(b, '.codex/state_5.sqlite', rows(b))


@case('codex-rollouts/sqlite-home-missing-dir', ['link', ERIN], tool='codex', mask=IDS)
def _(b):
    codex_world(b, 'sqlite_home = "%s"\n' % b.path('nowhere'))
    sessions(b, *FILES)
    db(b, '.codex/state_5.sqlite', rows(b))


for name, toml in [('config-invalid', 'sqlite_home = ['), ('sqlite-home-not-a-string', 'sqlite_home = 5\n'), ('config-not-utf8', b'sqlite_home = "\xff"\n')]:
    @case('codex-rollouts/%s' % name, ['link', ERIN], tool='codex', mask=IDS)
    def _(b, toml=toml):
        codex_world(b, toml)
        sessions(b, *FILES)
        db(b, '.codex/state_5.sqlite', rows(b))


@case('codex-rollouts/claude-link-leaves-it-alone', ['link', 'bob@example.com'], mask=IDS)
def _(b):
    codex_world(b)
    sessions(b, *FILES)
    db(b, '.codex/state_5.sqlite', rows(b))


@case('codex-rollouts/sqlite3-command-missing', ['link', ERIN], tool='codex', env={'PATH': '%HOME%/bin:/bin'}, tree=False, mask=IDS)
def _(b):
    # 这一例两边注定不一样（原来不靠外部命令，cca-core 找不到 sqlite3 就跳过）：只比输出和退出码，不比库
    codex_world(b)
    sessions(b, *FILES)
    db(b, '.codex/state_5.sqlite', rows(b))
