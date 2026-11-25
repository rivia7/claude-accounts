---
name: release
description: 发布 cca 的新版本（正式版或测试版）：定版本号、写更新说明、提交、发布前检查、在后台跑 release.sh、发布后核对。只由维护者输入 /release 触发。
argument-hint: "[版本号，比如 3.7.3 或 3.8.0-beta.1] [GitHub 上的更新说明]"
disable-model-invocation: true
---

# 发布 cca

规矩在根目录 CLAUDE.md 的「发布」；release.sh 每一步做什么、测试版的「先发后验」怎么做，在 `docs/dev/release.md`。这里是照着做的顺序。

只在维护者这台 Mac 上才有的（提交和打 tag 用的身份、推送用的凭证、签名密钥、从 worktree 发布时那份 release.sh 拷贝怎么改）不在仓库里，在维护者的发布笔记里（auto memory 的 `releasing-cca`）。读不到就停下来问。

## 1. 先说定发什么

- 参数里有版本号就照它；没有的话，正式版只加补丁号（3.7.2 → 3.7.3），测试版写成 `X.Y.Z-beta.N`。
- 发布会推送、在 GitHub 上建 release，撤回麻烦。先用一句话说出版本号和种类（正式版 / 测试版），等维护者回一句点名这个版本的话（比如「发布 3.7.3」）再往下走；`/release` 后面已经写了版本号的，就算点过名了。
- 「先发后验」只给测试版，而且只在维护者要求时用。

## 2. 发布前

1. 没有别的 release.sh、regress.sh 在跑：会话开始时 `tree-status.sh` 报过，再看一眼 `ps -axo pid,etime,command | grep -E '[r]elease\.sh|[r]egress\.sh'`。从现在到发布完，不改这份检出：release.sh 在回归跑完后才拷 `cca`，中途改的会进一个不含它的 tag。
2. 改 `cca` 里的 `VERSION`；在 `macos/Sources/CcaKit/Model/WhatsNew.swift` 的 `WhatsNew.all` 最前面加这一版的 `ReleaseNote`（标题平实，不写「正式版」；看看新手教程 `Guide.swift` 的 `TutorialCard` 要不要跟着改）。然后跑 `bash macos/test.sh`：更新说明的用词、版本先后在这里查，用正常优先级跑。
3. 提交：功能是一个提交，发布另一个（只改 `VERSION` 和 `ReleaseNote`，标题照以前的写成 `cca X.Y.Z：一句话`）。工作区要干净：`git status --porcelain` 有任何输出，release.sh 都不跑。
4. 核对：这次加进来的行里没有真实的 IP、节点名、邮箱；提交和 tag 的身份对（见发布笔记）；签名密钥在、权限是 600、和 `cca` 里的 `RELEASE_KEY` 是一对；四个目标都装了（在 `cli/` 里看 `rustup target list --installed`）；`git push --dry-run origin HEAD:refs/heads/main` 能过。

## 3. 跑 release.sh

- 放到后台跑 `./release.sh "更新说明"`（环境照发布笔记），约 20–25 分钟，大半是回归；最后那一行出来后可能还要停一两分钟，别杀它。
- 没过：照它说的那一句修，修好后从头再跑（附件放在退出时就删掉的临时目录里）。中途要停：停掉以后用 `git ls-remote --tags origin`、`gh release list` 确认 GitHub 上什么都没留下。

## 4. 发布后

- `git rev-parse v<版本>^{commit}` 和 HEAD、origin/main 是同一个；`gh release list` 里正式版是 Latest，测试版是 Pre-release。
- 下载 6 个附件核对：`install.sh` 和仓库里的一字节不差；`cca.sig` 在 `cca-release` 命名空间里验得过，`cca-app.zip.sig` 在 `cca-app` 里验得过、在 `cca-release` 里验不过；下载的 `cca` 在一个空的 HOME 里运行 `cca _gui hello`，报出新的版本号。
- 正式版发了以后，这一版之前的测试版，问过维护者再删（`gh release delete <tag> --yes --cleanup-tag`、`git tag -d <tag>`）。
- 改过 `server/` 的：Team 的服务器要在维护者 `cca update` 之后重新部署，先问。
- 改动要发版才会到已经安装的机器上，维护者自己的 Mac 也一样：请维护者自己运行 `! cca update`。
