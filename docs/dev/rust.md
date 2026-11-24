# 辅助程序（cca-core，`cli/` 下的 Rust 程序）

`cca` 的 bash 负责编排（问问题、起 claude / codex、和路由器说话），配置和账号表、共享链接、出口检测的解析、查用量、折合金额、设备统计……这些“算”的部分在辅助程序里：bash 里的 `py 子命令 参数…` 就是运行它。它原来是内嵌在 `cca` 里的 Python（`PY_SRC`，1.36 万行、149 个子命令），照原样移植成了 Rust：子命令、参数、标准输出、标准错误、退出码、写出来的文件都没变，所以 bash 那边一行调用都不用改。函数名也和原来的 Python 一样（`cmd_env`、`router_merge`、`cost_scan`……），别的文档里说“辅助程序的 `xxx`”，就在 `cli/src` 里按名字找：`grep -rn 'fn xxx' cli/src`。

换成原生程序图的是省：一次 `py` 调用从 45～78 毫秒（看用的是哪个 python3）、26～29 MB 内存降到约 3.5 毫秒、4 MB（一条 cca 命令要调十几二十次）；`cca` 启动时不再找 python3、不再把 60 万字的源码压缩打包；常驻的上报进程 31 MB → 5.7 MB。数字和量法在文末「量过的」。

## 文件在哪

| | |
|---|---|
| `cli/src/main.rs` | 入口：按第一个参数找子命令（`cmd::find`），没人接的错误说一句、退出码 1 |
| `cli/src/py/` | Python 兼容层：值（`obj::{V, Dict, List}`）、JSON、正则、文字、路径、文件、子进程、时间、摘要、属性列表、TOML、TLS、zlib、gzip……行为照 CPython（有当初从 CPython 3.14 取的对照数据：`cli/tests/vectors/*.json`，已经固定） |
| `cli/src/core/` | 地基：目录和常量（`base`）、三种工具（`tool`：原来的 `Tool` / `CodexTool` / `AgyTool`，`t()` 是当前工具）、账号表（`model`）、Team 的配置并进账号表（`policy`：名字是维护者的映射那时起的）、对账（`ctx`）、同步摘要（`sync`） |
| `cli/src/cmd/<模块>.rs` | 子命令，一个模块一份，每份末尾有自己的 `CMDS` 表（子命令名 → 函数、哪几种工具能用） |
| `cli/Cargo.toml`、`Cargo.lock` | 依赖是白名单（见下面「依赖」）；版本锁在 `Cargo.lock`，一律 `--locked` |
| `cli/.cargo/config.toml` | 从 macOS 交叉编译 Linux（musl 静态链接，用 Rust 自带的 rust-lld）；`cargo test` 起的进程里 `CCA_HOME` 指到不存在的目录 |
| `cli/src/cmd/pack.rs` | `cca-core pack`：把四种机器的辅助程序接到 `cca` 后面（发布、回归的 `mkver`、`tests/linux.sh` 用；原来是 `cli/pack.py`） |
| `cli/PORTING.md` | Python → Rust 的对照表和容易错的地方（`cli/src/py/` 的语义拿不准时查它） |
| `cli/FINDINGS.md` | 移植时在原实现里发现的问题（照原样留着的）和有意不一样的地方 |
| `cli/PORTED_FROM` | 最后一个还内嵌着 Python 的提交：回归里「换代的更新」拿那个提交的 `cca` 当旧版本；要看原来的 Python 也从它取（`git show $(cat cli/PORTED_FROM):cca`） |

## cca 怎么找到它

`cca` 开头的 `helper_find`（在定义 `py` 之前）定出 `HELPER`：

- **发布的 cca**：`release.sh` 用 `cca-core pack`（这台机器的那一份辅助程序自己）把四种机器的那一份（gzip 再 base64，各自包在一个 `: <<'CCA-HELPER-END'` 里）接在文件最后、`exit` 的后面——bash 读到 `exit` 就不往下读，`bash -n` 也照样通过；`HELPER_SUMS="目标=SHA-256 …"` 记着各自的校验值（对原样的程序算的：gzip 是 `py/gzip.rs` 用 miniz_oxide 压的，字节和原来 Python 的 zlib 压的不一样，`gunzip` 照样解得开；同样的输入出同样的字节）。Linux 的那两段还有一个用处：Team 的同步服务器（`cca-core team-server`、`team-acme`）要传到 Linux 服务器上的程序就是它们，`cmd/hub/payload.rs` 从这个文件里原样取出来（`cca team deploy` 的安装脚本、`cca team hosted` 的部署文件；见 team.md「安装脚本」），所以这几段的格式变了它也要跟着改。第一次运行时把这台机器的那一份解到缓存目录的 `cca-helper/<SHA-256>`（`helper_unpack`：`sed` 切出来、`base64` 解码、`gunzip`，校验值对了、试着运行过才 `mv` 换上；目录 700、只认自己的、不是链接；顺手删掉 30 天前解出来的旧版本和没解完的临时文件）。以后每次只看一眼“文件在不在、是不是我的、不是空的”，**不为它起任何进程**。解好的那一份中途没了（macOS 会清临时目录里几天没碰过的文件，而一个会话可以开很多天）：`py` 每次调用前看一眼在不在（内建的判断），不在就再解一次；这一份 `cca` 已经被新版本换掉的（更新之前起的会话），从更新留下的上一个版本 `~/.cca/update/cca.prev` 里解。macOS 上解的时候用系统自带的 `/usr/bin/base64`、`/usr/bin/shasum`（PATH 里排在前面的可能是 Homebrew 的 GNU base64，不认 `-D`）缓存目录：macOS 是这个用户自己的临时目录（`/var/folders/…/T/`，`TMPDIR` 不是它时问 `getconf DARWIN_USER_TEMP_DIR`），Linux 是 `${XDG_CACHE_HOME:-~/.cache}`（不放 `XDG_RUNTIME_DIR`：那是内存盘，退出登录就清空）。缓存没了（系统清理、重启）下次运行自己再解，约 0.1 秒；`cca uninstall`（`cca restore`）最后把缓存目录里的 `cca-helper` 整个删掉（见 uninstall.md）
- **仓库里的 cca**（`HELPER_SUMS` 空着）：用 `cli/target/release/cca-core`，没有就说先 `cd cli && cargo build --release`。改了 Rust 要重新编才生效（`tests/regress.sh` 开头会编）
- `CCA_HELPER=路径`：只对仓库里的 `cca` 管用（指定用哪一份：回归的 TIMING、`release.sh` 跑回归时用）。发布的 `cca` 不认它：一个环境变量不能让它去运行别的程序
- 目标按 bash 的 `HOSTTYPE`、`OSTYPE` 定（不起 `uname`；`HOSTTYPE` 认不出来时才问 `uname -m`：32 位的 bash 跑在 64 位系统上）：`aarch64` / `x86_64` × `apple-darwin` / `unknown-linux-musl`。别的机器说没有这种机器用的辅助程序（`install.sh` 下载之前也查）
- 解的时候要 `sed`、`base64`、`gunzip`、`shasum`（Linux 上 `sha256sum`）：`base64` 是这次新加的依赖（`need_deps`、`install.sh` 一起查，`pkg_hint` 认 `coreutils`）；python3 不再需要

## 命令

```
cd cli
cargo build --release --locked        # cca（仓库里的）用的就是这一份
cargo test --locked                   # 单元测试（回归的 unit 一节也跑）
cargo fmt                             # 照 rustfmt.toml 排版（回归里有 cargo fmt --check；生成的数据表标着 #[rustfmt::skip]，不参与）
cargo clippy --all-targets -- -D warnings
. ./musl-env.sh                       # 交叉编译 Linux 的两个目标之前（ring 里有 C 代码：指定编译器和归档工具；在 Linux 上什么都不做）
cargo clippy --all-targets --target x86_64-unknown-linux-musl -- -D warnings   # Linux 那一半的代码（连测试）也要干净
cargo build --release --locked --target x86_64-unknown-linux-musl   # 交叉编译；四个目标先 rustup target add
```

## 每次都要守的

- **bash 和辅助程序之间的约定不能悄悄变。** 子命令的参数、stdout 的格式、退出码是 bash（和界面 App 的接口）依赖的：改一边要同时改另一边，照 `macos/PROTOCOL.md` 的规矩（只能加字段）。stdout 只输出数据，提示走 stderr；`CCA_RO=1` 时不写文件、不往 stderr 说（`core::base::RO`、`notice`）
- **每次 `py` 都是一个新进程**，一条命令起十几二十次：启动要一直很轻（现在 2200 万条指令、2.5 MB，空的 Rust 程序是 1500 万条）。别在启动的路上加东西：系统框架不要写成 `#[link]`（`dock.rs` 的 CoreFoundation 是用到时才 `dlopen` 的——写成 `#[link]` 时每条子命令都多 1200 万条指令、3.6 MB）；正则用 `re!(…)`（用到才编译）；只有一条子命令用的表、文件别在 `main` 里读
- **外部命令照旧按名字起**（`curl`、`ssh`、`security`、`ps`、`ssh-keygen`、`scutil`、`osascript`……，参数顺序别动）：回归把它们换成 PATH 上的假命令并核对收到的参数。探针、控制接口的 curl 第一个参数是 `-q`，路由写明（`leak_curl_argv` 有测试）
- **起子进程别让标准库 fork**：一律经 `py::proc::Cmd`（自己用 `std::process::Command` 的，起之前调 `proc::cloexec_inherited()`），不要 `pre_exec`。子进程里要做事时标准库只能 fork，而 macOS 上多线程的进程 fork 的那一刻别的线程正在做 libSystem 只做一次的初始化（`getaddrinfo` 第一次用到 libnotify）的话，子进程在 fork 自己的收尾里就被杀掉（SIGKILL，`~/Library/Logs/DiagnosticReports/cca-core-*.ips` 里是 `crashed on child side of fork pre-exec`）：泄露检测的 STUN 线程查地址时别的线程起 curl，偶尔就少一个请求（3.6.2 以前回归的 3d 常挂在这上面）。所以 bash 交过来的 3、9 号在父进程里标成 exec 时关掉（`cloexec_inherited`），给了新环境的命令照新环境的 PATH 先找好路径（`resolve`），标准库就用 posix_spawn；只有 stderr 并进 stdout、新会话（托管内核的 `keep.rs`、远程会话起转接进程的 `remote/host.rs` 也是）、伪终端（`remote/pty.rs`）这几种还要 fork
- **密钥不进命令行和环境变量**：经 stdin 交给 `security -i`、经 600 的文件。日志、出错的说明里也不带
- **不碰真的 `~/.cca`**：单元测试自己建临时目录、把路径当参数传（`cargo test` 起的进程 `CCA_HOME` 指到不存在的目录）；钥匙串在测试里只走 `secrets::testing::set_security` 装的假的
- **值的语义照 Python**（字典记着键的先后、`True == 1`、`x or 默认值`、按字符不按字节、两种取整……）：`cli/PORTING.md` 的对照表和「容易错的地方」。输出要逐字节稳定的地方（JSON、摘要的输入、给路由器的账号表）尤其要照它
- **信号**：处理函数只置一个标记，会阻塞的地方（`signal::sleep`、等子进程、kqueue）醒来以后 `signal::check()?`；常驻的进程（上报、托管内核、盯邮箱）靠它收场
- **依赖是白名单**（下面）。`unsafe` 只在和系统打交道的地方（`libc`、`dlopen`），每一处写 `// SAFETY:`

## 依赖

只用这几个常见的纯 Rust 库，加新的之前先问维护者：`libc`、`regex`（关掉了用不着的特性）、`sha1`、`sha2`、`miniz_oxide`（zlib、gzip：折合金额和别的设备交换的那一份是压缩的，发布的 `cca` 后面接着的辅助程序是 gzip 压过的）、`unicode-normalization`；`rustls` + `ring`（TLS 和签名，四种机器的构建都带：Team 的同步服务器、证书助手跑在 Linux 上，要 TLS、验设备的签名、给证书机构的请求签名；邮箱助手连 Gmail 的 IMAP 只在 macOS 上有，证书照系统的 `/etc/ssl/cert.pem` 验。ring 里有 C 和汇编：从 macOS 交叉编译 Linux 的两个目标前先 source `cli/musl-env.sh`）。HTTP 一律经 `curl`（和原来一样），不带 HTTP 库。JSON、属性列表、TOML、YAML 的一小部分、base64、URL、IP 地址都是 `cli/src/py/` 里自己照 CPython 写的（有对照数据）。

发布的四份辅助程序（2026-11-25，Team 的同步服务器和证书助手编进去以后）：`aarch64-apple-darwin` 6.4 MB、`x86_64-apple-darwin` 7.6 MB、`x86_64-unknown-linux-musl` 8.5 MB、`aarch64-unknown-linux-musl` 6.8 MB（`opt-level = 2`、整体 LTO、`panic = "abort"`、去掉符号；四份都带 TLS）；接上以后发布的 `cca` 约 20 MB（内嵌 Python 的时候 0.9 MB），Team 的同步服务器的安装脚本约 10 MB（带着 Linux 的那两份），大小由 `release.sh` 打出来。服务端编进去之前是 4.7、5.6、5.3、4.1 MB，`cca` 约 13 MB。试过的减小体积的办法（没采用，要先整套验证）：依赖库按体积优化（`[profile.release.package.<库>] opt-level = "s"`）再去掉 `regex` 的 `perf` 特性，一份少 11%（4680 → 4179 KB），一次调用的用时没有变化；连自己的代码也按体积优化是少 24%。`release.sh` 编的时候带 `--remap-path-prefix`，程序里不留编译机器的路径。

## 测试

- `cargo test`：兼容层对着 CPython 的对照数据（`tests/vectors/`）、各模块的内部函数（从外面运行辅助程序查不到的都在这里）
- `tests/*.rs`（测试程序 `suite` 的各个模块，回归的 unit 一节收它们的结果）：把辅助程序当黑盒跑（`kit::harness` 的 `helper` 给出它在哪：`CCA_HELPER`，没有就是 `cli/target/release/cca-core`）。临时 HOME、PATH 上放假命令；单独跑之前先 `cargo build --release`，再 `cargo test --profile kit -p cca-tests --test suite -- 模块名`（见 tests/CLAUDE.md）
- `tests/regress.sh`：整条 `cca` 命令的行为。开头先编辅助程序；10g 里测“怎么找到辅助程序”（没编出来、`CCA_HELPER`、第一次解出来、不再解、清旧的、文件不完整、校验值不对、缓存目录是链接、没有这种机器的、缺 `base64`、两个同时第一次运行）；`mkver` 做的假发布版像真的一样接着辅助程序
- 移植时用过的对拍（原来的 Python 和 cca-core 在同一份沙箱上各跑一遍、逐字节比）和从原来的 Python 生成对照数据的脚本已经删了：以后的开发直接照 Rust 来，行为用 `cargo test`、`tests/*.rs`、回归测。`cli/tests/vectors/` 里从原来的 Python 取的那几份（`src_*.json`、`usage.json`、`corenet.json`、`clients.json`、`mail.json`）成了固定的数据，有意改了行为就直接改它们或者改测试；只照 CPython 标准库生成的表（`cli/src/py/uni_tables.rs`、`xml_tables.rs`、`zip_cp437.rs`、`cmd/mail/html5.rs`、`cmd/clients/casefold_table.rs`）和对照数据（`cli/tests/vectors/` 里其余的几份）也固定了：生成它们的脚本（`cli/tools/gen_*.py`）跟着仓库里别的 Python 一起删了，要看去 git 历史里翻。兼容层的行为以这些数据为准，不再跟着 CPython 的新版本走
- Linux：`tests/linux.sh` 用 musl 的那一份（`--docker` 时在宿主机上先交叉编译；见 `docs/dev/linux.md`）

## 量过的

同一台 Mac（Apple 芯片），各自热身以后取平均；原来的 Python 照 `cca` 的办法起（编译缓存在、用真正的 python3）。整条 `cca` 命令的那几行是在回归的沙箱里量的（装好了、有账号、路由器同步过；claude、curl、ssh 这些都是假的，两边一样，所以剩下的时间里有一截是假命令自己的）：CPU 连子进程一起算。换完以后一条命令的时间大头在 bash 自己身上（解析 6800 行、`locale charmap`、`getconf`、几十个小的外部命令），那是下一步的事。

| | 原来（Python） | 现在（Rust） |
|---|---|---|
| 一次 `py` 调用（`py email '别名(邮箱)'`，最小的一条） | Homebrew 的 python 3.14：45 毫秒（CPU 42 毫秒），29 MB，4.8 亿条指令；命令行工具带的 3.9：78 毫秒（CPU 74 毫秒），26 MB，8.8 亿条 | 3.5 毫秒（CPU 2.4 毫秒），4.0 MB（实占 2.2 MB），2600 万条；不认识的子命令（只有启动）2200 万条、2.4 MB |
| `cca version`（量的是 cca 自己的启动） | 179 毫秒 | 37 毫秒 |
| `cca ls` | 537 毫秒（CPU 524） | 102 毫秒（CPU 87） |
| `cca _gui hello` / `cca _gui state` | 230 / 302 毫秒 | 85 / 87 毫秒 |
| `cca _gui live` | 714 毫秒（CPU 667） | 389 毫秒（CPU 339） |
| `cca usage --json` | 1229 毫秒（CPU 1166） | 462 毫秒（CPU 411） |
| 启动一个账号（`cca @alice`：到 claude 起来，再到它退出后收尾） | 3.25 秒（CPU 2.10 秒） | 1.69 秒（CPU 0.52 秒） |
| 上报进程（`cca _presence-agent`，常驻） | 31.0 MB（实占 21 MB），到第一次上报 CPU 140 毫秒 | 5.7 MB（实占 3.5 MB），65 毫秒 |
| 折合金额第一次全读（10 万条回复、416 MB 会话记录） | CPU 1.46 秒，52 MB | CPU 0.69 秒，11 MB |
| 折合金额之后没有新增时 | 0.15 秒，60 MB | 0.03 秒，13 MB |

折合金额的去重表（`scan.json` 的 `seen`，35 天里每条回复一项）在内存里是 16 个字节一项的紧凑表（`cost.rs` 的 `Seen`），直接从文件读、直接写回去，写出来的文件和原来逐字节一样。
