# 远程会话的网页端（手机浏览器里的那一页）

服务器（`cca-core team-server --web-url …`）把这个目录里的文件原样发给浏览器：只发 `.html`、`.js`、`.css`、`.png`、`.svg`、
`.webmanifest`，这份说明、许可证、校验和不发。没有打包、压缩这一步；文件是编进辅助程序的（`cli/src/server/web.rs` 的 `FILES`），改了
要重新编（`cd cli && cargo build --release --locked`），开发时用 `--web-root 这个目录` 让服务器直接读这里的。设计、怎么测写在 `docs/dev/remote.md`「网页端」，
线上的格式在 `server/README.md`「网页端」。

| 文件 | 是什么 |
|---|---|
| `index.html`、`app.css`、`app.js` | 界面：会话的列表、配对、设置、终端和按键条 |
| `core.js` | 协议和加密（配对、签名、通道、接入、断了重新接上）；没有界面，Node 里也能跑（`tests/web_client.mjs`） |
| `logo.svg`、`icon-*.png`、`manifest.webmanifest` | Logo、添加到主屏幕以后的图标和名字 |
| `xterm.js`、`xterm.css`、`xterm-addon-fit.js`、`xterm-LICENSE.txt` | 终端：xterm.js（别人的代码，见下） |

## xterm.js

原样取自 npm 上发布的包（MIT 许可，`xterm-LICENSE.txt`），一个字节没改：

| 文件 | 包 | 包里的路径 | SHA-256 |
|---|---|---|---|
| `xterm.js` | `@xterm/xterm` 5.5.0 | `lib/xterm.js` | `1f991ac3b4b283ebf96e60ae23a00a52765dd3a2e46fa6fdda9f1aab032f7495` |
| `xterm.css` | `@xterm/xterm` 5.5.0 | `css/xterm.css` | `ba8e6985669488981ccf40c0cefe3aba80722cb6c92de7ad628b0bd717faf2b6` |
| `xterm-addon-fit.js` | `@xterm/addon-fit` 0.10.0 | `lib/addon-fit.js` | `bdaefa370b1bfc42ee88d46fe6072400902a4d4b2d45cd93438dda9b23c97089` |
| `xterm-LICENSE.txt` | `@xterm/xterm` 5.5.0 | `LICENSE` | `b569f629d00f2626a8100df2a1798210535621e42164dfd426a6fe5aac7b0ccd` |

两个包的完整性校验值（npm 的 `dist.integrity`，下载下来的压缩包算出来和它一样）：

- `@xterm/xterm@5.5.0`：`sha512-hqJHYaQb5OptNunnyAnkHyM8aCjZ1MEIDTQu1iIbbTD/xops91NB5yq1ZK/dC2JDbVWtF23zUtl9JE2NqwT87A==`
- `@xterm/addon-fit@0.10.0`：`sha512-UFYkDm4HUahf2lnEyHvio51TNGiLK66mqP2JoATy7hRZeXaGMRDr00JiSF7m63vR5WKATF605yEggJKsw0JpMQ==`

换版本：`npm pack @xterm/xterm@版本 @xterm/addon-fit@版本`，核对 `dist.integrity`，换掉这四个文件，改上面的表（`tests/web_relay.rs`
的 `the_real_page` 照这张表核对文件），在真的浏览器里走一遍（同一份里的 `the_page_in_a_browser`）。没用 6.x：它把滚动换成了自己画的滚动条，
手机上手指滚动靠不靠得住没有把握；5.5.0 是浏览器自己的滚动。只载入了这两个文件：没有图片、剪贴板（OSC 52）、网页链接这些插件，
会话的输出让终端做不了这些事。页面的内容安全策略不许 `eval`，这两个文件里也没有。

## 图标

`icon-*.png` 是 `macos/tools/make-icon` 画的（字形和 App 的图标是同一份 `LogoGlyph.swift`），铺满、不裁圆角：

```
swiftc -O -swift-version 5 macos/tools/make-icon/main.swift macos/Sources/CcaApp/LogoGlyph.swift -o /tmp/make-icon
/tmp/make-icon --flat /tmp/flat.png && for n in 180 192 512; do sips -z $n $n /tmp/flat.png --out server/web/icon-$n.png; done
```

`logo.svg` 是同一个字形照 `LogoGlyph.swift` 里的坐标和颜色手写的：那边改了这里跟着改。
