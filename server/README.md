# cca 的设备统计服务器（presence）

App 账号页里的“设备”一节来自这里：映射里的账号在某台 Mac 上经 cca 使用时（命令行会话、`cca app` 打开的桌面 App），那台 Mac 会报告给这台服务器。

- 部署和更新：在 cca 的仓库里运行 `./policy.sh presence deploy root@主机`。它把这个目录装到服务器的 `/root/projects/cca-presence`，`--dir` 可以换位置。然后运行 `docker compose up -d`，把地址、证书公钥的指纹和盐写进映射目录的 `policy.json`，再推送设备清单。之后照常 `./policy.sh 映射目录` 发布映射。
- 设备清单：`./policy.sh presence sync`。它把各台机器的 deploy key 用发布密钥签名（命名空间 `cca-devices`）推给服务器，`./policy.sh key add/rm/mv` 之后会自动推。服务器只认这份清单里的机器。
- 状态：`./policy.sh presence status`；服务器上的日志：`docker compose logs`。

## 服务器上的目录

| 路径 | 内容 |
|---|---|
| `compose.yaml` | 用官方的 `python:3.13-slim` 镜像，不用构建 |
| `.env` | `CCA_PRESENCE_PORT`：对外的端口，默认 18443 |
| `app/cca-presence.py` | 服务端：只用 Python 标准库，自己验 SSHSIG 签名（RFC 8032），不需要 OpenSSH |
| `config/release.pub` | 维护者的发布公钥 |
| `certs/` | 自签的 P-256 证书和私钥，私钥不出服务器 |
| `data/state.json` | 各台设备最近一次的报告，以及每个账号上次使用的时间（保留 30 天） |

## 迁移

1. 把整个目录拷到新服务器（`certs/` 一起拷，映射里钉住的指纹就不用改）。
2. 在新服务器上运行 `docker compose up -d`。
3. 在 cca 的仓库里对新服务器运行一次 `./policy.sh presence deploy root@新主机`：它会改好映射里的地址，然后发布映射。

## 隐私

服务器能看到的只有这些：

- 各台设备的公网 IP、设备名（deploy key 的别名）。
- 账号的摘要 HMAC(盐, 工具 + 邮箱)。
- 哪个账号在什么时候、开着几个会话、闲置了多久。

它看不到邮箱，也看不到会话内容。每个请求都不记日志。
