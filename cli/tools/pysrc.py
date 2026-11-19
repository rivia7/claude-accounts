"""原来内嵌在 cca 里的 Python（PY_SRC）在哪取：生成对照数据的那几份脚本（gen_*_vectors.py）共用。

仓库里的 cca 还带着它（主干上）就取那一份；切换以后的 cca 里没有了，从 cli/PORTED_FROM 记的那个主干提交里取
（`git show 提交:cca`，和对拍 cli/pydiff/run.py 的 oracle_cca 一样）。"""
import os
import subprocess

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
HEAD = "PY_SRC=$(cat <<'PY'\n"
_TEXT = []


def cca_text():
    """带着 PY_SRC 的那一份 cca 的全文"""
    if not _TEXT:
        with open(os.path.join(REPO, 'cca'), encoding='utf-8') as f:
            text = f.read()
        if HEAD not in text:
            with open(os.path.join(REPO, 'cli', 'PORTED_FROM')) as f:
                rev = f.read().split()[0]
            got = subprocess.run(['git', '-C', REPO, 'show', rev + ':cca'], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            if got.returncode != 0:
                raise SystemExit('取不出带着 PY_SRC 的 cca（git show %s:cca）：%s' % (rev, got.stderr.decode('utf-8', 'replace').strip()))
            text = got.stdout.decode('utf-8')
        _TEXT.append(text)
    return _TEXT[0]


def py_src():
    """PY_SRC 的全文"""
    return cca_text().split(HEAD, 1)[1].split('\nPY\n)', 1)[0]


def definitions():
    """PY_SRC 里的定义（不带最后的子命令分派）"""
    return py_src().split('\nif len(sys.argv) < 2', 1)[0]
