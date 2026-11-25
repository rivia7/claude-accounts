---
paths:
  - "cli/src/cmd/team.rs"
  - "cli/src/cmd/team/**"
  - "cli/src/cmd/hub.rs"
  - "cli/src/cmd/hub/**"
  - "cli/src/server/**"
  - "server/**"
  - "tests/team.rs"
  - "tests/team_relay.rs"
  - "tests/hub_deploy.rs"
  - "tests/acme.rs"
  - "tests/kit/hubbox.rs"
  - "tests/kit/acmefake.rs"
  - "tests/kit/spec.rs"
  - "tests/kit/spec/**"
  - "tests/tool/hubbox.rs"
  - "tests/tool/acme_fake.rs"
  - "tests/tool/spec.rs"
  - "tests/vectors/spec_wire.json"
  - "cli/tests/vectors/team_doc_ct.json"
  - "macos/Sources/CcaKit/Protocol/Team.swift"
  - "macos/Sources/CcaKit/Protocol/Hub.swift"
  - "macos/Tests/Fixtures/team_devices.json"
  - "docs/dev/team.md"
---

# Team 的成对修改

从根目录 CLAUDE.md「要一起改的」分出来的，只跟 Team 有关：同步服务器、一键部署、托管平台、证书助手、线上格式、节点、界面里的 Team。读到上面 `paths` 里的文件时自动加载；只改 `cca` 里 bash 那一侧（`cmd_team`、`team_deploy`、`hub_deploy_ask`、`hub_deploy_run` 这些）时，自己先来读这一份。Team 怎么实现、怎么测见 `docs/dev/team.md`；远程会话经 Team 的服务器中转的那些在 `.claude/rules/remote.md`。

- 部署到托管平台（`cca team hosted`）：服务端 `CCA_HUB_HOSTED=1` 的预设（`cli/src/server/cli.rs`：端口 8080、`/data/state.json`）↔ `cli/src/cmd/hub/hosted.rs` 的 `team_files` 生成的 Dockerfile（`FROM scratch`，里面只有一个 `cca-core`：`payload.rs` 给的 x86_64 那一份）、步骤、README ↔ server/README.md「托管平台」
- Team 的同步服务器的一键部署（`cca team deploy`）：`server/hub-install.sh` ↔ `cli/src/cmd/hub/deploy.rs`：前面放进的几项（`CCA_HUB_MANAGED`、`CCA_HUB_HOST_HINT`、`set -- …`，里面的 `--name` 照 `shq` 转义）、结果行（`report_managed` 的 `CCA-HUB 键 值` ↔ `report`）、原因码（`die` 的 ↔ `fail_code` ↔ bash `team_fail` 给的 `team_…` ↔ PROTOCOL 4.14）、`--dir` 的规矩（`dir_check` ↔ `dir_of`，共用的证书助手的目录 `AD` ↔ `ACME_DIR`）、网页端没开的原因（`WEB_WHY` ↔ `report` 的 `web_off` ↔ bash `team_deploy` 的 `case`：`port80`、`acme_old`、`lan`、`host`、`web_port`）；安装脚本带着的服务端程序（两种机器的 `cca-core`，gzip + base64，各在一个 here-document 里）：`deploy.rs` 的 `installer` 写的 `CCA_HUB_VERSION='…'`、`CCA_HUB_CORES='…'`、`CCA_HUB_CORE_SHA256_<机器名>='…'` 这几行和函数 `hub_core_<机器名>`（结束行 `CCA_HUB_CORE_<机器名>_EOF`）↔ `hub-install.sh` 的 `core_pick` / `core_put` ↔ release.sh 的 `core_ok` ↔ regress 11b3d 的 `hbcore`（照它们取出来核对）；机器名：`cli/src/cmd/hub/payload.rs` 的 `LINUX` ↔ `cca-core pack` 的 `TARGETS` ↔ `hub-install.sh` 的 `core_arch`（照 `uname -m` 认）；Docker 方式的容器用的空镜像：`hub-install.sh` 的 `RIMG`（`cca-team-runtime:1`，`rimg` 用 `docker import` 在本机建，不拉镜像）↔ `tests/kit/hubbox.rs` 的 `RUNTIME`（假的 docker 照它查 compose.yaml）；网页端的文件编在服务端程序里（`cli/src/server/web.rs` 的 `FILES` ↔ `server/web/` 里服务端给出去的文件），安装脚本不带；以前的版本（3.6.x，服务端是 Python 的）装的要能就地换过来、它的安装脚本也要能在这一版装的上面再运行：`hub-install.sh` 里清掉以前的文件的那一行、`.env` 里不再写的 `CCA_HUB_IMAGE` ↔ `tests/hub_deploy.rs` 的 `make_python_era`；证书助手（`cca-core team-acme`）的参数（`--teams`、`--store`、`--out`、`--account`、`--listen`，Docker 方式另有 `--ca /app/ca.pem`：容器里没有 CA 证书，`acme_start` 把服务器上的那一份拷进去；CA 证书在哪：`hub-install.sh` 的 `ca_bundle` ↔ `cli/src/server/acme/roots.rs` 的 `SYSTEM`，它找的那几个都要在 `ca_bundle` 里、先后一样；它放进 `out/<服务名>/` 的两个文件名）↔ `hub-install.sh` 起它的那几处（几个 Team 共用的 `cca-team-acme`：`acme_compose`、`acme_unit_file`，目录 `/opt/cca-team-acme`、`teams/<服务名>` 的登记）↔ `tests/kit/hubbox.rs` 的 `acme_stub`、`stage`（假服务器只起 `app/cca-core team-server`，`team-acme` 换成假的证书助手）；Team 的名字：`hub-install.sh` 写的 `app/name` ↔ 服务端的 `--web-name-file`（`web_name`、`web_named`：标题、`apple-mobile-web-app-title`、manifest 的 `name` / `short_name`）↔ `server/web/index.html`、`manifest.webmanifest` 里被换掉的那几处；App 里部署前查登录的原因码（`ssh_trouble`：`ssh`、`ssh_auth`、`ssh_denied`、`ssh_keyonly`、`ssh_hostkey`）↔ bash `hub_deploy_ask` 的 `case`；安装脚本的模板是 `include_str!` 编进辅助程序的，改了要重新编
- Team 的线上格式（`team_*` 的请求、邀请链接、从邀请里那串随机数派生的令牌 / 设备编号 / 密钥 / 公钥的认证码、每台设备的配置的密文和签名）：`server/README.md`「Team 的同步服务器」↔ 服务端（`cca-core team-server`，`cli/src/server/`：请求的检查在 `wire.rs`、存的在 `store/team.rs`）↔ `cli/src/cmd/team/doc.rs` ↔ `tests/kit/spec.rs`（照文档独立实现的，`cca-test spec` 在回归里用它；`doc.rs` 测试里的对照数据、`cli/tests/vectors/team_doc_ct.json` 和它自己的 `tests/vectors/spec_wire.json` 都是固定的数据，没有重新生成它们的命令：改了格式两边的代码和这几处数据一起换，`tests/kit/spec/vectors.rs` 的 `the_helpers_team_doc_vector` 看着两边封出来的密文一样）；服务器的上限（`cli/src/server/mod.rs` 开头的常量）↔ 辅助程序里写死的同一个值（`MAX_TEAM_DOC` ↔ `doc.rs` 的 `DOC_MAX`，`SEQ_MAX`，`MAX_TEAM_BATCH` ↔ `admin.rs` 的 `BATCH`，`MAX_ACCOUNTS` ↔ `cost.rs` 的 `COST_BATCH`）；配置明文的样子（`doc_problems`）↔ docs/dev/team.md「设备拿到的配置」
- Team 的同步服务器的状态文件名（`HubKind::file` 加 `team.<编号>.`）↔ `cli/src/cmd/team.rs` 的 `drop_synced`（退出、被移除时删掉的前缀）↔ App 的 `CcaWatcher` 盯的文件（`teamFile`：`presence/team.<编号>.status.json`、`cost/team.<编号>.remote.json`、`sync.json`）↔ PROTOCOL 1 的那份清单 ↔ `cca restore` 的 `--exclude`（`cost/team.*.remote.json`）
- Team 的一键部署：`server/hub-install.sh` 的 Team 模式（`CCA_HUB_TEAM_ROOT`、`--team-root`、服务名 `cca-team-<编号的前 8 位>`、`.env` 的 `CCA_TEAM_ROOT`）↔ `cli/src/cmd/hub/deploy.rs` 的 `team_install`（前面放进的几项、目录默认 `/opt/<服务名>`）↔ `cmd/team/admin.rs` 的 `cmd_team_deploy`（服务名）↔ 服务端（`cli/src/server/cli.rs`：`CCA_TEAM_ROOT` 两种写法、托管的预设）↔ `cmd/hub/hosted.rs` 的 `team_files` ↔ `tests/kit/hubbox.rs`（按服务名分的 unit、进程号文件：`systemctl_run`、`pid_file`）；bash 里部署用的那几段（`DEPLOY_PY` 等几个变量，`hub_deploy_ask`、`hub_deploy_run` 这些函数：名字里的 hub 是 3.5.2 部署自己的同步服务器时起的）↔ `team_deploy`
- Team 在界面里：PROTOCOL 2.1 `hello.team`、2.2 `config.teams` / 账号的 `team`、4.14 ↔ `cli/src/cmd/team.rs` 的 `team_rows`、`admin.rs` 的 `cmd_team_devices`（`--json`）↔ Swift 的 `Protocol/Team.swift`（`TeamAPI.version` ↔ bash `gui_hello` 的 `"team":1`）↔ `macos/Tests/Fixtures/team_devices.json`、`state.json` ↔ regress 11b3d 的 `$T/guifx4`
- Team 的命令：bash 的 `cmd_team`（`py team-*` 的参数、一行一项的 stdout、原因码）↔ `cli/src/cmd/team.rs`、`team/admin.rs`；同意项 `team_leave`、`team_delete`、`team_revoke`、题号 `team.join`、结果和失败的代码：`cca` ↔ PROTOCOL 4.14（4.2、4.6）↔ Swift 的 `ConsentItem`、`ResultCode`；`~/.cca/teams/` 是凭证：`CCA_DIR_PRIVATE` 的 `teams/**` ↔ `cca restore` 的 `--exclude`（`teams/*/secret`、`root_ed25519`、`admin.json`、`doc.json`）和 `restore_teams`
- Team 的节点（`cca team node`）：实测的临时内核的入站名 `cca-test-N` ↔ `tests/setup.sh` 的假 mihomo（认 `"cca-test-1"`）
- 证书助手（`cca-core team-acme`，`cli/src/server/acme/`）↔ `tests/kit/acmefake.rs`（照 RFC 8555 / 8738 和证书种类的草案核对它的请求；改了流程两边一起看）；它写的文件名（`fullchain.pem`、`privkey.pem`）↔ 服务端的 `--web-cert` / `--web-key`；Let's Encrypt 的规矩（短期证书那一种的名字 `shortlived`、续的时机、限额）变了要改 `acme/mod.rs` 开头的说明和 `RETRY`、`args.rs` 的 `fraction`
