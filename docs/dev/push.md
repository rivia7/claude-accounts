# 推送（APNs）

iPhone 上的 cca App 锁着屏、切到别的 App、没开着时，远程会话里的通知（cca 自己带的钩子推的「做完了」「在等你」、`cca remote notify`）也送得到：
经苹果的推送服务（APNs）。用户定的（2026-11-26）：推送服务的密钥只放在维护者自己的服务器上（推送网关，`cca-core team-push`），各个 Team 的
服务器把推送请求转给它；通知的内容到手机上才解开，一路上的服务器都看不到明文；cca 在远程会话里自己带上「做完了 / 在等你」的钩子
（docs/dev/remote.md「cca 自己带的钩子」）。

线上的格式（`PUSH` 帧、`push_set`、网关的请求、通知的密文）在 server/README.md「推送」；iOS 那一头（通知扩展、钥匙串分组、登记）在
docs/dev/ios.md「推送」。

## 一条通知怎么走

1. 会话里的钩子（`cli/src/cmd/remote/hook.rs`）、`cca remote notify` 经本机的 socket 说 `NOTIFY_REQ`；宿主（`host.rs`）限流（`NOTE_BURST`、
   `NOTE_GAP`）、去重（一样的字 `NOTE_SAME` 秒内只一次），给这一条编个号（`Note::id`，16 个小写十六进制字符），`NOTIFY` 发给接着的各处
2. 转接进程（`link.rs` 的 `push_note`）给每个让看这个会话、对方是手机（`peer.web`）的配对封一份（`crypto.rs` 的 `note_seal`：只有这个配对的
   P 派生的通知密钥解得开），控制连接上发 `PUSH`（`0x0a`，`{"pid","id","note"}`）给这个会话走的 Team 的服务器。手机这时接没接着都发：
   接着的那份（`NOTIFY`）和推来的那份带着同一个编号，手机上只弹先到的那一份
3. Team 的服务器（`cli/src/server/push.rs`）：会话、配对对得上，配对的另一头是登记了推送的手机（`push_set`），就 POST 给推送网关（`--push`，
   钉住 `--push-pin` 的公钥）。一条线程、有界的队列（中转的线程不等它，满了丢）；每台手机限流（`PHONE_BURST`、`PHONE_GAP`）；网关回 410
   （令牌没用了）就拿掉那个登记
4. 推送网关（`cli/src/server/gateway.rs`、`gateway/apns.rs`、`gateway/h2.rs`）：签好令牌（JWT，ES256，45 分钟换一次）交给推送服务（HTTP/2，
   正式、开发两种环境各一条连接），推送里只有固定的一句（标题 `cca`、正文「远程会话里有新的通知」）、`mutable-content: 1` 和密文 `n`；
   `apns-collapse-id` 是通知的编号
5. 手机：通知扩展（`ios/Notify`）拿钥匙串里的通知密钥解开、换上真的字（标题、「来自「机器名」 · 副标题」、正文：和 App 自己弹的一样，
   CcaRemoteKit 的 `NoteCard`）；解不开（不是配给这台手机的、取消了配对）照固定的那一句显示

谁知道什么：
- 宿主：明文（本来就是它的）
- Team 的服务器：哪台手机（推送令牌、环境）、哪个配对、什么时候、多长（明文补齐到 256 字节的整数倍）；解不开
- 推送网关：令牌、环境、编号、密文；不知道是哪个 Team、哪个会话（只看得到来的是哪台服务器）；解不开
- 苹果：令牌、固定的那一句、密文
- 手机上的通知扩展：只拿得到通知密钥（`Kn = HMAC(P, "cca-remote/v1/note")` 派生的两把），拿不到 P：解得开通知，解不开终端、冒充不了配对

跟用户说过的两点（2026-11-26）：
- 推送令牌是一台手机上这个 App 的固定编号：同一台手机登记在几个 Team 的服务器上，这几台服务器（和网关）看得出是同一台手机
- 网关不认谁能用：知道它的地址和一台手机的令牌就推得过去，只是推过去的手机解不开、照固定的那一句显示；每个 IP、每个令牌各一个限流的桶
  挡着刷屏。只给同一台服务器上的 Team 用时，安全组里不放行网关的端口，就只有那台服务器上连得上

## 推送网关（`cca-core team-push`）

`--cert`、`--key`（P-256 的自签证书：Team 的服务器只认这把公钥，证书机构、名字、有效期都不看）、`--apns-key`（开发者账号里建的 .p8：只有
这个程序读它，不进命令行、环境变量）、`--key-id`、`--team-id`（都是 10 个字符），`--listen`（默认 `0.0.0.0:18460`），`--topic`（默认
`com.cca.remote`），`--ca`（Docker 的空容器里没有系统的 CA 证书：挂一份进去指给它）；`--print-pin` 只打出证书的公钥（Team 的服务器的
`--push-pin`）。`GET /v1/health` 回 `{"v":1,"ok":true}`。一个连接一个请求，最多 `CONNS` 个连接；推送服务那头的连接闲着久了换新的、
它说 GOAWAY 了换新的、流量控制照它给的窗口（`tests/push_gateway.rs` 都测了）。

## 部署

### 网关（维护者的服务器上，只有一份）

要维护者自己做的：在苹果开发者账号里建推送服务的密钥（Certificates, Identifiers & Profiles → Keys → 新建，勾上 Apple Push Notifications service，
环境选 Sandbox & Production），下载 `AuthKey_<Key ID>.p8`（只能下载一次，自己留一份），放到服务器上网关的目录里（`keys/AuthKey.p8`，600，属主是
跑网关的用户）。Claude 不读、不传这个文件。

目录照这台服务器上别的项目的样子（一个项目一个目录、compose 起；镜像用 Team 的安装脚本在本机建的空镜像 `cca-team-runtime:1`）：

```
/opt/cca-push/
  app/cca-core     Linux x86_64 的辅助程序（同一台服务器上 Team 的目录里 app/cca-core 那一份，要 3.7.3 以上：有 team-push）
  app/ca.pem       这台服务器的 CA 证书拷一份（Team 的安装脚本 ca_bundle 找的那几个地方）
  certs/           openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 36500 -subj /CN=cca-push \
                     -keyout certs/key.pem -out certs/cert.pem（私钥 600）
  keys/AuthKey.p8  维护者放的
  compose.yaml
```

```yaml
services:
  cca-push:
    image: cca-team-runtime:1
    container_name: cca-push
    restart: unless-stopped
    user: "10001:10001"
    command: ["/app/cca-core", "team-push", "--listen", "0.0.0.0:18460", "--cert", "/certs/cert.pem", "--key", "/certs/key.pem",
              "--apns-key", "/keys/AuthKey.p8", "--key-id", "<Key ID>", "--team-id", "<Team ID>", "--ca", "/app/ca.pem"]
    ports:
      - "18460:18460"
    volumes:
      - ./app:/app:ro,z
      - ./certs:/certs:ro,z
      - ./keys:/keys:ro,z
    read_only: true
    cap_drop: [ALL]
    security_opt:
      - no-new-privileges:true
    mem_limit: 128m
    pids_limit: 128
```

起来以后 `docker run --rm -v /opt/cca-push/certs:/certs:ro … /app/cca-core team-push --cert /certs/cert.pem --print-pin`（或者在服务器上直接运行
那个程序）打出公钥。还没有安装脚本：只有一份，手动装；要升级时换 `app/cca-core`、`docker compose up -d --force-recreate`。

### Team 的服务器

`cca team deploy Team 服务器 --push https://网关:18460 --push-pin sha256//…`（安装脚本的 `--push`、`--push-pin`，记在 `.env` 的
`CCA_PUSH_URL`、`CCA_PUSH_PIN`：再部署时不写照上次的，`--no-push` 去掉）。systemd 方式的服务经 `EnvironmentFile` 拿到这两项，Docker 方式的
在 compose 的 `environment` 里（`@PUSH@` 那两行）；要开着中转（没开时记着、不交）。起来时日志里说一句「推送：……转给推送网关 主机:端口」，
`/v1/health` 的 `relay.push` 是 true。和网关在同一台服务器上的 Team 用内网地址（云服务器的公网 IP 多半是 NAT 进来的，从里面连自己的
公网 IP 不一定通）。别的服务器上的 Team 要用的话，网关的地址和公钥由维护者给（还没定怎么给：可以发布时写进 cca，或者写在文档里）。

## 测试

- 辅助程序的单元测试：`server/push.rs`、`gateway.rs`、`gateway/apns.rs`（JWT、推送的内容）、`gateway/h2.rs`（HPACK、帧）、`cmd/remote/crypto.rs`
  （`note_seal` 的向量）、`cmd/hub/deploy.rs`（`--push` 的参数、结果行的 `push`）
- `tests/push_gateway.rs`：网关对着假的推送服务（`kit::apnsfake`：照 RFC 9113 / 7541 自己写的 HTTP/2 服务端，照苹果的文档核对每个请求——
  ALPN、头、JWT 的签名、内容；故意用几种写法回头；回的结果由令牌的最后两个十六进制字符定）
- `tests/web_relay.rs` 的 `app_guest_gets_pushes`：真的宿主 → Team 的服务器 → 网关 → 假的推送服务那一整条
- `tests/ios_kit.rs`：`against_the_spec` 的 note 向量（CcaRemoteKit ↔ `kit::spec` 的 `note_seal`）；`pushes_reach_the_phone`（ccaguest 登记、
  会话里 `remote-notify`、假的推送服务收到、CcaRemoteKit 解开，取消配对以后解不开）；`in_the_simulator`（`CCA_IOS_SIM=1`：真的 App 登记、
  通知密钥写进共用的钥匙串分组，App 照通知扩展的读法解开真的宿主封的那一份）
- `tests/hub_deploy.rs` 的 `push`：安装脚本、`cca team deploy` 的 `--push`、`--push-pin`、`--no-push`，`.env`，交给服务端的两种方式
- `tests/remote_relay.rs` 的 `cca_hooks_reach_the_other_machine`：cca 自己带的钩子推到别的机器

## 没验证的

- 真的推送服务：要维护者建了密钥、部署了网关才测得到（假的推送服务是照苹果的文档写的）
- 通知扩展这个进程：模拟器里的 `xcrun simctl push` 不经它（2026-11-26 试过：系统日志里是 CoreSimulatorBridge 直接把通知加进通知中心，
  `mutableContent: YES` 也不调扩展，到 App 的还是固定的那一句）。扩展里解开的那一段和 `in_the_simulator` 里 App 照扩展的读法解开的是同一份代码
  （CcaRemoteKit 的 `openPushed`、`NoteCard`、`NoteKeychain.read`），可扩展的进程读不读得到共用分组的钥匙串、时限内做没做完，要真的推送
  到真机（TestFlight 的构建）上才看得到
- 环境：TestFlight / App Store 装的照描述文件的 `aps-environment` 认成 production，Xcode 装的是 sandbox；真机上还没看过
