# `cli/src/py/` 的语义：Python → Rust 对照和容易错的地方

`cca` 里原来内嵌的 Python（`PY_SRC`，约 1.36 万行，149 个子命令）已经照原样移植成 `cli/` 下的 Rust 程序 `cca-core`，`cca`
从 3.5.0 起用的就是它（bash 编排没动：照旧 `py 子命令 参数…`）。移植时用的对拍和从原来的 Python 生成对照数据的脚本已经删了，
以后的开发直接照 Rust 来；辅助程序平时怎么开发、怎么测，看 `docs/dev/rust.md`。

这份文件留下的是 `cli/src/py/` 那一层的语义：移植出来的代码处处照 Python 的值和行为写（字典记着键的先后、`True == 1`、
`x or 默认值`……），新代码挨着它们写、输出要和以前逐字节一样时照样要守。原来的 Python 在 `git show $(cat cli/PORTED_FROM):cca`
里（`PY_SRC=$(cat <<'PY'` 到 `PY` 结束符，找函数用名字：`grep -n '^def cmd_env'`），想知道某段代码当初为什么这么写时去翻。
移植时发现的原实现的问题和有意不一样的地方记在 `cli/FINDINGS.md`。

- 子命令：在所属模块（`cli/src/cmd/<模块>.rs`）里写 `pub fn cmd_xxx(args: &[String]) -> R<()>`，加进那个模块末尾的 `CMDS`
  （`Cmd::all(名字, 函数)`：三种工具走同一个函数，差别在 `core::tool::t()`）。
- 外部命令照旧按名字起（`py::proc::Cmd`，同样的参数顺序；写了绝对路径的照写绝对路径）：回归把它们换成 PATH 上的假命令并核对参数。
- `cargo test` 起的进程里 `CCA_HOME` 指到一个不存在的目录（`.cargo/config.toml`）：要读写文件的测试自己建临时目录、把路径当参数传；
  靠 `CCA_HOME` 下有东西的逻辑用 `tests/*.py`（把辅助程序当黑盒跑）或回归测。

## Python → Rust 对照（`cli/src/py/`）

| Python | Rust |
|---|---|
| 字典、列表、JSON 的值 | `py::obj::{V, Dict, List}`：动态的值。`Dict`、`List` 是共享的引用（`clone` 不拷贝，和 Python 的赋值一样），字典记着键的先后。`dict!{"k" => v}`、`list![a, b]` |
| `d.get(k)` / `d[k]` / `k in d` | `d.get(k)`（`Option<V>`）/ `d.at(k)`（没有就 panic，等于没人接的 KeyError）/ `d.has(k)` |
| `d[k] = v` / `d.pop(k, None)` / `d.setdefault` | `d.set(k, v)` / `d.pop(k)` / `d.setdefault(k, v)` |
| `for k, v in d.items()` | `for (k, v) in d.items()`（快照，循环里可以改 d） |
| `isinstance(x, str)` / `dict` / `list` | `x.as_str()` / `x.as_dict()` / `x.as_list()`；从字典里取：`d.str(k)` / `d.dict(k)` / `d.list(k)` |
| `isinstance(x, int)`（布尔值也算）/ `type(x) is int` | `x.isinstance_int()` / `x.is_int()`、`x.as_int()` |
| `if x:` / `x or y` | `x.truthy()` / `x.or(y)`；`d.get(k) or 默认` 常写成 `d.get(k).filter(|v| v.truthy())` |
| `a == b`（`True == 1 == 1.0`，字典不看顺序） | `a.py_eq(&b)` |
| `str(x)`、`'%s' % x`、`print(x)` / `repr(x)` | `x.py_str()` / `x.py_repr()`（`None`、`True`、列表的写法都照 Python） |
| `json.loads` / `json.dumps(x)` | `py::json::loads` / `dumps(&v, Fmt::new())`；`ensure_ascii=False` → `.utf8()`，`indent=2` → `.indent(2)`，`separators=(',', ':')` → `.compact()`，`sort_keys=True` → `.sort_keys()`。常用的两种：`json::FILE`、`json::LINE` |
| `load_json` / `load_quiet` / `save_json` | `py::fs::{load_json, load_quiet, save_json}` |
| `open(p).read()` | `py::fs::read_text`（不是 UTF-8 是 `Fail::Value`；`\r\n` 换成 `\n`） |
| `re.compile(r'…')`、`.match` / `.search` / `.fullmatch` / `.sub` / `.findall` / `.split` | `re!(r"…")`（正则照 Python 的原样写）、`.match_` / `.search` / `.fullmatch` / `.sub` / `.findall` / `.split`；只要真假：`.matches`、`.found`。不带 `re.M` 的 `$` 照 Python 也匹配结尾换行之前。前瞻、后顾、反向引用不支持：改成手写的判断，并写测试 |
| `s.strip()`、`s.split()`、`s.splitlines()`、`len(s)`、`s[:n]`、`s[-n:]` | `py::text::{strip, split_ws, splitlines, len, head, tail}`（认的空白、断行的地方、按字符数都照 Python）。`s.split(',')`、`startswith`、`replace` 用 Rust 自带的 |
| `s.isdigit()`、`int(s)`、`float(s)` | `text::isdigit`、`text::to_int`、`text::to_float`（认不出是 `None`，原来没接 ValueError 的地方要自己转成 `Fail::Value`） |
| `shlex.quote` / `shlex.split` | `text::sh_quote` / `text::sh_split` |
| `dw(s)`、`fit(s, w)` | `py::uni::width`、`text::fit` |
| `round(x)` / `round(x, n)` / `int(x + 0.5)` / `int(x)` / `a // b` | `py::num::{round, round_to, half_up, trunc, floordiv}`。`round` 是四舍六入五成双，和 `int(x + 0.5)` 不是一回事，原来用哪个就用哪个 |
| 小数写成文字 | `py::num::float_repr`（别用 `format!("{}", f)`：`1e-05`、`1.0` 的写法对不上） |
| `print(…)` / `sys.stderr.write(… + '\n')` | `out!(…)` / `err!(…)`；不换行的 `out_raw!` / `err_raw!`。**不要用 `println!` / `eprintln!`**：stdout 要像 Python 那样整块缓冲 |
| `sys.stdout.flush()` | `py::io::flush_out()`：原来在哪 flush，就在哪 flush |
| `sys.stdin.read()` / `.readline()` | `py::io::{read_stdin_text, read_stdin_line}`（不转换 `\r\n`） |
| `die(msg)` / `sys.exit(n)` | `return die(msg)` / `return exit(n)`（都是 `Err`，`?` 往上传） |
| `except (OSError, ValueError):` | `Err(e) if e.is_os_or_value()`；`FileNotFoundError` → `e.is_not_found()`；自己的异常类 → `Fail::Other("类名", …)` 和 `e.is("类名")` |
| `subprocess.run([...], capture_output=True, timeout=30)` | `py::proc::Cmd::new([...]).capture().timeout(30.0).run()` → `Done { rc, stdout, stderr }`；超时是 `ProcError::Timeout`，找不到命令是 `ProcError::Os`。`stdin=DEVNULL` → `.stdin(In::Null)`，`stdout=DEVNULL` → `.stdout(Out::Null)`，`input=数据` → `.input(数据)`，`start_new_session=True` → `.new_session()`，`Popen` → `.spawn()`。子进程不带 3 号以上的文件描述符（和 Python 默认的 `close_fds=True` 一样，`Cmd` 已经做了） |
| `os.makedirs(p, exist_ok=True)` / `os.makedirs(p, mode=0o700, exist_ok=True)` | `py::fs::makedirs` / `makedirs_mode`（只有最里面那一层用给的权限，和 Python 3.7 起一样） |
| `put(path, data, mode)`（内容没变不写）、`shutil.rmtree`、`os.utime(p)`、`os.fsync`、`tempfile.TemporaryDirectory()` | `py::fs::{put, rmtree, utime_now, fsync, TempDir}` |
| `set(x or [])` / `sorted(…)` / `x in (y or [])`（值可能被手改成别的类型） | `cmd::secrets::{py_set, py_sorted, py_in}` |
| `os.environ.get(k)` | `py::proc::getenv(k)` |
| `os.path.*` | `py::path::*`（`realpath` 遇到不存在的路径不报错，和 Python 一样） |
| `time.time()`、`localtime`、`strftime`、`mktime`、`calendar.timegm` | `py::time::*` |
| `hashlib.sha1/sha256(…).hexdigest()`、`hmac`、`os.urandom`、`uuid.uuid4()` | `py::hash::*`、`py::rand::*` |
| `urlsplit`、`parse_qsl`、`quote`、`unquote`、`ipaddress` | `py::url::*`、`py::ipaddr::*`（规则是 CPython 3.14 的，有对照数据） |
| `base64.b64encode` / `b64decode(s)` / `b64decode(s, validate=True)` / `urlsafe_b64decode` | `py::base64::{b64encode, b64decode(s, false), b64decode(s, true), urlsafe_b64decode}`；出错是 `Fail::Value`（binascii.Error 是 ValueError 的子类），说明和 CPython 一样 |
| `plistlib.load` / `loads` / `dumps(v)` / `dumps(v, fmt=FMT_BINARY)` | `py::plist::{loads, dumps_xml, dumps_binary}`，值是 `py::plist::P`（不是 `V`：属性列表有数据和日期）。输出和 plistlib 逐字节一样；异常类对得上（ExpatError 不是 ValueError：`e.is("ExpatError")`）。和 Python 不一样的三处写在文件头 |
| `tomllib.loads` | `py::toml`（值是 `V`；错误的说明和 tomllib 一样，有对照数据） |
| `xml.etree.ElementTree.fromstring` | `py::xml::fromstring`（`Tree::root()`、`El::{tag, text, get, children, findall, find, findtext}`；带 DOCTYPE 的一律不收，只认四种编码） |
| `zipfile.ZipFile` | `py::zip::ZipFile`（`infolist`、`open_member`、`read`；只读，不支持 bzip2 / lzma / zstd） |
| `zlib.compress` / `decompress`、`ssl`（TLS 客户端） | `py::zlib`、`py::tls`（只有 macOS 的构建带） |
| `signal.signal(sig, 处理函数)` | `py::signal::{raise_on, check, sleep}`（见下面「信号处理函数里 raise 的」） |
| `open(path)` 失败时的 `str(e)` | `py::fs::open_error_text(&e, path)`（带文件名的 repr） |
| `with Lock():` | `let _lock = core::base::Lock::acquire()?;`（离开作用域解锁；不能重入） |
| 全局的 `T`、`with using(tool):` | `core::tool::t()`、`let _g = Using::tool(tool);` |
| `ctx()`、`cfg_load()`、`cfg_save(c)`、`T.state(c)` | `core::ctx::{ctx, cfg_save}`、`core::model::cfg_load`、`tool.state(&c, create)` |
| `sh(K=v)` / `sh_arrays(N=[…])` | `core::base::{sh, sh_vals, sh_array}` |
| `notice(code, text, k=v)` | `core::base::notice(code, text, dict!{…})` |

兼容层里新加东西（写在 `cli/src/py/` 下）照同样的规矩：行为照 CPython，期望值从真的 Python 取（`cli/tools/gen_vectors.py`
那种对照数据最好）。

## 别的模块里已经有的

要用别的模块的函数，先 `grep -rn 'pub fn 名字' cli/src` 找（名字和原来的 Python 里一样）。除了各模块的子命令，
这些常被别处用到的已经有了：

- `cmd::secrets`：`kc_*`、`ks_read / ks_write / ks_del`、`api_secret`、`security`、`keyfile_store`、`keystore_name`
- `cmd::presence`：`presence_problems`、`presence_load`、`presence_key`、`presence_mapped`、`presence_wake`、`spawn_background`、`PRESENCE_DIR`
- `cmd::keyshare`：`keyshare_on`、`keyshare_edit`、`keyshare_kick`、`KeyshareFile`、`keyshare_sum`、`keyshare_slot`、`is_num`
- `cmd::mail`：`MAIL_SVC`、`MAIL_ADDR_RE`、`MAIL_DOMAIN_RE`、`mail_one`、`mail_all_addrs`、`mail_forget_orphans`
- `cmd::session`：`ps_rows`、`proc_scan`、`proc_starts`、`gui_procs`、`cli_marks / cli_alive / cli_view`、`profile_path`、`private_file`、`lstart_epoch`
- `cmd::gui`：`health_file`、`health_write`、`health_result`、`ver_key`；`cmd::usage`：`parse_ts`；`cmd::codex`：`codex_probe`
- `cmd::launchers`：`subprocess_quiet`、`launcher_names`、`launcher_badges`、`launcher_tool`
- `cmd::router`：`device_id`、`router_merge`、`parse_node_list`、`node_match`、`nodes_rename`、`nodes_record`、`node_users_all`
- `cmd::accounts`：`cmd_env` 用的 `api_env`、`api_settings`、`bypass_list`、`Tool::flags / run_args`，`edit_sum`、`env_text`、`env_parse`、`node_users`
- `cmd::table`：`need`、`account_named`、`all_errors`
- `cmd::usage`：`usage_cached`、`usage_row`、`usage_entry`、`usage_weeks_view`、`span`、`ago`、`day_gap`；`cmd::codex`：`codex_claims`、`agy_claims`
- `cmd::corenet`：`slot_orders`、`local_names`、`local_info`、`local_want`、`core_defs`、`udp_view`、`udp_leak_hint`、`write_text`
- `cmd::cost`：`CostView`；`cmd::mail`：`mail_cfg`、`mail_view`、`mail_ready`；`cmd::clients`：`cu_claude_channel`、`cu_claude_env`

## 容易错的地方

- **原来顺序不定的地方。** Python 遍历集合（`set`）的先后每次运行都不一样（字符串的哈希是随机的）。输出、起命令的先后靠它时，
  Rust 里排好序，在注释里说一声。
- **`except Exception` 接不住 `die()`，但接得住别的。** 原来 `try: … except Exception: 退一步` 的地方：`Fail::Die / Exit` 往上传，
  `Fail::Os / Value / Other` 走退一步的那条路（见 `cmd_sync_state`）。原来把 stderr 接到 /dev/null 的，照样接（`QuietStderr`）。

- **字典的顺序就是输出的顺序。** 生成 JSON、打印、算摘要都靠插入顺序；`d[k] = v` 已有的键留在原位，`pop` 后再 `set` 排到最后。
  Python 里先建一个新字典再赋回去的，照样新建，别在原字典上改（顺序会不一样）。
- **摘要的输入要逐字节一样。** 凡是 `hashlib`、`hmac` 的输入：`json.dumps` 的参数（`ensure_ascii`、分隔符、`sort_keys`）、
  拼接用的分隔符、有没有换行，都照原来的。
- **`x or 默认值` 不等于“没有才用默认值”。** 空串、0、空列表也会落到默认值。
- **布尔值是整数。** `isinstance(True, int)` 为真；原来特意排除布尔值的地方（`isinstance(v, bool) or not isinstance(v, int)`）照样排除。
- **按字符，不按字节。** `len`、切片、截断、宽度都是按 Unicode 字符；Rust 的 `s.len()`、`&s[..n]` 是字节。
- **`str.splitlines()`、`str.split()`、`str.strip()` 认的字符比 Rust 的多**（`\x0b`、`\x1c`–`\x1e`、`\x85`、U+2028 等），用 `py::text` 的。
- **`print` 一个不是字符串的值** 是 `str()` 的写法：`True`、`None`、`['a', 'b']`。
- **取整有两种**（见上表）。`'%d' % 小数` 是截断。
- **原来没接的异常是“调用栈 + 退出码 1”。** Rust 里让它成为 panic 或往上传的 `Fail::Value` / `Fail::Os` 即可（main 会说一句、退出码 1）。
- **有副作用的顺序。** 先写了哪个文件、先说了哪句话、出错时已经改了什么，都照原来的先后。
- **`except Exception: pass` 接不住 `die()`**（`SystemExit` 不是 `Exception`）；`except SystemExit` 才接得住。
- **只读模式。** `CCA_RO=1` 时不写文件、不往 stderr 说（`core::base::RO`、`notice` 已经照做）；新加的会写文件的对账逻辑照这个规矩。
- **文件的权限和原子替换。** 原来用 `save_json(…, 0o600)`、`O_EXCL`、`os.replace`、`follow_symlinks=False` 的地方照样；密钥类文件 600、目录 700。
- **密钥不进命令行和环境变量。** 原来经 stdin 交给 `security -i`、经文件交的，照样。测试的假命令也别把密钥打到 stdout 以外的地方。
- **线程。** `V` / `Dict` / `List` 不能跨线程（里面是 `Rc`）：交给线程之前取成 `String`、`Vec`、数字。
- **时间。** 本地时间的换算一律经 `py::time`（C 库的 `localtime` / `mktime`），别自己算时区。
- **正则。** 把模式原样搬过来就行；注意 `re.match` 只管开头、`re.search` 哪里都行、`re.fullmatch` 要整串。
  同一个模式在循环里用的写成 `re!(…)`（只编译一次）。
- **`T` 是隐含的参数。** `adir`、`cdir`、`acct_json`、`kc_*` 这些都看当前工具；原来 `with using(x)` 里调的，Rust 里也要在 `Using::tool(x)` 的作用域里调。
- **信号处理函数里 `raise` 的**（上报进程的 `Stop`、等登录时的 `KeyboardInterrupt`）：用 `py::signal`。`raise_on(信号, "异常类")`
  装上，会阻塞的地方（`signal::sleep`、等子进程、kqueue）被打断以后 `signal::check()?` 把它当异常传上去；`py::proc` 等子进程时
  看到信号来了会杀掉子进程、返回 `ProcError::Interrupted`（那一笔留着，调用方接着 `check()?`）。原来 `except Stop:` 的地方看
  `e.is("Stop")`。Python 是任意两条字节码之间都可能冒出来，Rust 只在这些检查点：检查点之间别留下“被打断才会有”的
  副作用（把被打断的请求当成失败记下来之类），所以每个会被打断的子进程调用后面紧跟一个 `check()?`。
- **`except SystemExit: pass` 接住 `die()` 的地方**：原来 `die()` 抛出之前就把说明写到 stderr 了，Rust 里说明带在 `Fail::Die` 里、
  到 main 才写。接住不再往上传的地方调一下 `e.swallowed()`，说明才不会丢。
- **`with 上下文管理器:` 里的 `return` / `continue`** 也算正常退出，`__exit__` 照样做收尾（`KeyshareFile` 就是在这时写回文件的）。
  Rust 里把 with 的身体写成一个返回 `R<T>` 的闭包，出来以后按 `is_ok()` 收尾（`keyshare.rs` 的 `with_keys`）。
- **运行时拼出来的正则**（`re.compile(a + re.escape(b))`）：Python 的 `re.compile` 有缓存，常驻进程里每一轮都用的不会重新编译。
  Rust 里用 `py::re::compile(写法)`（带同样的缓存）；转义用 `py::re::escape`。

