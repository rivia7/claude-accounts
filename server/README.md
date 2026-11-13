# cca 的设备统计服务器（presence）

App 账号页里的“设备”一节来自这里：映射里的账号在某台 Mac 上经 cca 使用时（命令行会话、`cca app` 打开的桌面 App），那台 Mac 会报告给这台服务器。
官方账号记下的 API key（`cca key 账号`、App 的「记录 API key…」）也经它同步给登录了这个账号的其他设备：设备只传密文，服务器存着转交，解不开。
App 的「折合金额」（按官方 API 标价折算的用量）也一样：每台设备算自己的，经它交换密文，账号的合计才包括所有设备。

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
| `data/keys.json` | 各个账号记下的 API key 的密文（每个账号一份，新的换掉旧的）和最后传它的设备；设备撤销后不删（是账号的） |
| `data/cost-slots.json` | 折合金额的密文：每个账号、每台设备一份（设备只能写自己的那一份）；设备撤销后不删（是账号的用量），100 天没再传的删掉（`--cost-days`） |

## 迁移

1. 把整个目录拷到新服务器（`certs/` 一起拷，映射里钉住的指纹就不用改）。
2. 在新服务器上运行 `docker compose up -d`。
3. 在 cca 的仓库里对新服务器运行一次 `./policy.sh presence deploy root@新主机`：它会改好映射里的地址，然后发布映射。

## 隐私

服务器能看到的只有这些：

- 各台设备的公网 IP、设备名（deploy key 的别名）。
- 账号的摘要 HMAC(盐, 工具 + 邮箱)。
- 哪个账号在什么时候、开着几个会话、闲置了多久。
- 每个账号记下的 API key 的密文、哪台设备什么时候传的。
- 每个账号在每台设备上的折合金额的密文（大小大致随用了多少个小时变）、什么时候传的。

它看不到邮箱，也看不到会话内容和 API key。每个请求都不记日志。

API key 的密文：密钥由映射里的盐、邮箱和账号的登录身份（Claude 的 accountUuid、ChatGPT 的 chatgpt_user_id）用 HMAC-SHA256 派生
（`root = HMAC(盐, "cca-keyshare-v1\0" 工具 "\0" 邮箱小写 "\0" 身份)`，加密用 `HMAC(root, "enc")`、认证用 `HMAC(root, "mac")`；
盐用映射里那串十六进制解出来的字节（部署时生成的是 32 个字符、16 个字节），工具是 `claude` 或 `codex`，字符串都按 UTF-8），
只有登录过这个账号的设备算得出来。内容是 `{"v":1,"key":key 或 null（删掉了）,"at":记下的时间}`，用空格补齐到 256 字节的整数倍（看不出是 key
还是删除、key 有多长），以 `HMAC(enc, nonce + 8 字节的块号)` 当密钥流异或加密（nonce 是随机的 16 个字节，块号从 0 起、大端，每块 32 字节），
再以 `HMAC(mac, "cca-keyshare-v1\0" 账号摘要 "\0" nonce 密文)` 认证（账号摘要是上报用的那 32 个十六进制字符）；
传的是 `{"n": nonce, "c": 密文, "t": 认证}`（base64）。谁新谁旧、真的假的由设备按认证过的时间自己判断：别的设备传上来旧的、伪造的，
登录了这个账号的设备不认，还会把自己的传回去。

折合金额的密文（op `ledger`，`/v1/ledger`）：密钥照上面的算，只是标签换成 `cca-cost-v1`（`root = HMAC(盐, "cca-cost-v1\0" 工具 "\0" 邮箱小写 "\0" 身份)`），
和 API key 的互不相通。内容是 `{"v":1,"at":传的时间,"h":{整点的时间戳:[金额（百万分之一美元）, 请求数, 输入, 输出, 缓存读, 缓存写, 其中没有标价的请求数]},"um":[没有标价的模型…]}`
（只有这台设备上这个账号最近 14 周的），zlib 压缩，前面加 4 个字节（大端）的压缩后长度，用零字节补齐到 1024 字节的整数倍，再照上面的办法加密；
认证是 `HMAC(mac, "cca-cost-v1\0" 账号摘要 "\0" 设备指纹 "\0" nonce 密文)`（设备指纹是 deploy key 的 `SHA256:…`，挪不到别的账号、别的设备上）。
请求是 `{"put": {账号摘要: 密文}, "get": [账号摘要…], "have": {账号摘要: {设备指纹: 认证码}}}`：put 只写这台设备自己的那一份，
get 回各台设备的 `{fp, name, me, revoked, tag, age, blob}`（have 里认证码没变的、这台设备自己的不带 blob），`missing` 是 30 天内经 cca 用过这个账号、
却没传过折合金额的设备（多半是 cca 太旧）。一份密文最长 98304 个字符，一台设备最多 128 个账号、4 MB，一次回的密文最多 512 KB（其余的下次再取）。

更新服务器：旧版的服务器不收 API key（cca 每 6 小时再试一次，App 里说要维护者更新服务器）。在 cca 的仓库里再运行一次 `./policy.sh presence deploy root@主机` 就换上新的 `cca-presence.py`，`data/` 里原有的不动。旧版的服务器也不收折合金额（App 里只有这台设备的，说要维护者更新服务器），同样这样更新。早先试过的 `/v1/cost` 是另一种格式，新版不再认（回 404，那时的客户端照旧版服务器处理），服务器上留下的 `data/cost.json` 可以删掉。
