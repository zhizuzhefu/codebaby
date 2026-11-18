# requestbaby

> AI 交付的独立验收层，辅助人对 AI 交付的成果验收。在接口边界（HTTP / gRPC / WebSocket）和真实界面（Web / Android，每一步截图留证）核对行为，一个 JSON 文件一个用例；命令行给 AI 和 CI 跑，桌面端给人审、给人改，两边读写同一个目录，结论一致。

```
 ____                             _         _         ____
|  _ \ ___  ___ _ __     _ __ ___| |_ _   _| | ___   / ___|__ _ _ __ ___   ___
| |_) / _ \/ __| '_ \   | '__/ _ \ __| | | | |/ _ \ | |   / _` | '_ ` _ \ / _ \
|  _ <  __/\__ \ |_) |  | | |  __/ |_| |_| | |  __/ | |__| (_| | | | | | |  __/
|_| \_\___||___/ .__/   |_|  \___|\__|\__,_|_|\___|  \____\__,_|_| |_| |_|\___|
               |_|
```

## 它解决什么

AI 交付得越来越快，人没法逐行审代码。requestbaby 是 AI 交付的**独立验收层**：不看实现，只在接口边界核对行为，辅助人对 AI 交付的成果做验收。

- **独立**：用例只写「发什么请求、应得什么响应」，与实现代码分开；AI 改得了代码，绕不开接口行为
- **人做决定**：用例是普通 JSON 文件，进 Git、能 review；人审几十行用例定下约定，不必逐行审实现
- **结论可复现**：`requestbaby verify` 一条命令跑完全部用例与流程，退出码 0 / 1 / 2 就是结论；每次运行连同等价命令写进共享历史，CLI、桌面端、CI 看到同一份证据

## 5 个概念

| 概念 | 是什么 | 在哪 |
|------|--------|------|
| 用例 | 一个请求 + 它应得的响应 | `collections/**/NN-名称.json`，文件名序 = 执行序 |
| 集合 | 一个目录；公共请求头、标签 | 目录里的 `collection.json`（可选） |
| 环境 | 一组变量：`baseUrl`、账号 | `envs/<环境>.json` |
| 断言 | 响应应该是什么样 | 用例的 `expect` |
| 流程 | 按顺序跑几个集合 | `requestbaby.json` 的 `workflows` |

## 看懂一个用例

```json case
{
  "name": "登录",
  "request": {
    "method": "POST",
    "url": "{{baseUrl}}/auth/login",
    "body": { "json": { "username": "{{username}}", "password": "{{password}}" } }
  },
  "expect": {
    "status": 200,
    "body": {
      "code": 0,
      "data.token": { "type": "string" }
    }
  },
  "capture": { "token": "data.token" }
}
```

读法：向 `环境里的 baseUrl + /auth/login` 发 POST；期望状态码 200、响应体 `code` 等于 0、`data.token` 是字符串；
把 `data.token` 存成变量 `token`，后面的用例写 `{{token}}` 就能用。失败时报告直接写明：`body.code = 0 → 期望 0，实际 4030001`。

## HTTP、gRPC、WebSocket、Web、Android

一个用例对应一种协议，请求部分五选一；断言、捕获、变量、集合、流程、报告完全一样。

| 协议 | 请求写在 | `expect.status` | `expect.headers` | `expect.body` |
|------|----------|-----------------|------------------|---------------|
| HTTP | `request`：method、url、headers、body | 状态码 200 | 响应头 | 响应体 |
| gRPC | `grpc`：target、method、message、metadata | `"OK"` / `"NOT_FOUND"`… | 响应元数据 | 响应消息（流式为数组） |
| WebSocket | `ws`：url、headers、steps（发送 / 接收并断言 / 等到某条消息…） | 握手状态 101 | 握手响应头 | 收到的全部消息 |
| Web | `web`：url、steps（点击 / 填写 / 断言…） | 页面的 HTTP 状态 | 页面响应头 | 结束时的 `{url, title}` |
| Android | `android`：app、steps（点按 / 填写 / 滑动 / 断言…） | 无 | 无 | 结束时的 `{package, activity}` |

gRPC 用服务端反射或项目里的 .proto 文件（`requestbaby.json` 的 `grpc.protos`），不需要生成代码；WebSocket 的每一步都能断言、捕获，
后面的步骤直接用捕获的值。详见 `requestbaby guide grpc` / `requestbaby guide ws`。桌面端按协议切换各自的编辑界面与响应视图。

Web / Android 用例按用户看得到的东西定位（`{"role": "button", "name": "登录"}`、`{"label": "手机号"}`），
每一步自动等待、**自动截图**（点了哪里、验证到哪个元素都框出来），失败时另存页面结构、控制台、网络请求 / logcat；
证据放在 `.requestbaby/runs/<运行 ID>/`，运行记录绑定代码提交，`run --report html` 生成带截图时间线的单文件报告。
Web 直接驱动本机 Chrome / Edge（DevTools 协议，不需要 Playwright / Node），也能连 Docker / 云端的远程浏览器；Android 只需要 adb，远程模拟器 / 云手机写 `主机:端口` 自动连接；运行与判定不调用大模型、不消耗 token。
详见 `requestbaby guide web` / `requestbaby guide android`。

需要代理时一处设置、全部生效：`requestbaby proxy set socks5h://主机:端口`（或桌面端「项目设置 → 代理」），HTTP / gRPC / WebSocket / 浏览器 / adb 都经它，`proxy off` 一键直连。详见 `requestbaby guide proxy`。

## 界面与文件一一对应

| 桌面端 | 文件里 |
|--------|--------|
| 用例编辑器「请求」页 | `request`（方法、URL、参数、请求头、请求体） |
| 「断言」页：状态码 / 响应体表 / 响应头表 | `expect.status` / `expect.body` / `expect.headers`（一行 = 一条） |
| 「断言」页：捕获为变量 | `capture` |
| 「脚本（进阶）」页 | `pre` / `post` / `tests`（JS，表格写不出的才用） |
| 「高级」页 | `tags`、`skip`、超时、数据驱动 `cases` |
| 侧栏「流程」/ 流程编辑器 | `requestbaby.json` 的 `workflows` |
| 环境管理页 | `envs/*.json` |
| 响应面板「字段」页：断言值 / 断言类型 / 捕获… | 往 `expect.body` / `capture` 里加一条 |

每个操作都有等价命令（悬停查看或一键复制，如 `requestbaby send collections/demo/01-health.json -e dev`），保存的文件与 `requestbaby fmt` 的格式逐字节一致。
在桌面端改、在编辑器里改、让 AI 改，是同一件事。

## 安装

```bash
cargo install --locked --path crates/requestbaby-cli    # 命令行 requestbaby
make mac-app && make install-mac                         # macOS 桌面端（本机签名）→ /Applications/requestbaby.app
```

构建需要 rustup 与 cmake（编译内嵌 libcurl）。运行时零外部依赖：libcurl 静态链接、TLS 用 rustls、证书走系统证书库。
正式发布：`make mac-release`（universal + Developer ID 签名 + 公证 → dmg，需要钥匙串证书与 notarytool 凭证）。

## 上手

```bash
requestbaby init myapi && cd myapi          # 项目骨架：dev / prod 环境 + 示例集合 + AGENTS.md
requestbaby vars set baseUrl http://localhost:8080 -e dev
requestbaby new case collections/orders 创建订单 -m post
requestbaby send collections/orders/01-创建订单.json --suggest   # 看真实响应，照着生成 expect / capture
requestbaby run                              # 跑全部用例
requestbaby verify                           # 校验 → 全部用例 → 全部流程，一个结论
requestbaby open                             # 在桌面端打开
```

已有资产：`requestbaby new case <目录> --curl "curl …"`（浏览器 Copy as cURL）、`requestbaby import openapi.json`（OpenAPI 3 / Swagger 2 / Postman v2.1）。

## 写偏了会被提醒

`requestbaby validate` 除了报错，还给写法建议：长 JS 断言可以写成 expect、post 里的取值可以写成 capture、URL 写死了域名……
带 `fixable` 的由 `requestbaby fmt --fix` 一键改写（语义不变，桌面端用例编辑器底部「一键改写」同一实现）。

## 和 AI 一起用

告诉 AI：「接口测试用 requestbaby，先执行 `requestbaby guide`」。它会读到一张 6KB 的速查卡，读完就能写出一次通过校验的用例；
需要时再看 `requestbaby guide <主题>`。`requestbaby init` 生成的 `AGENTS.md` / `CLAUDE.md` 已经写好这条约定。

文档三层，都编译在二进制里、与所装版本一致：

| 层 | 命令 | 内容 |
|----|------|------|
| 速查卡 | `requestbaby guide` | 5 个概念、典型用例、expect / capture、闭环命令、规矩 |
| 场景配方 | `requestbaby guide <主题>` | `expect` `capture` `vars` `flow` `auth` `e2e` `data` `setup` `env` `grpc` `ws` `web` `android` `proxy` `scripts` `cli` |
| 字段参考 | `requestbaby guide ref` | 由 JSON Schema 生成的全部字段（`requestbaby schema --all` 是原始 Schema） |

源文件在 [docs/guide/](docs/guide/)；其中每个示例都由测试真实执行（validate + dry-run），文中每条命令都由测试核对存在。

## CI

```yaml
- run: requestbaby verify -e ci --report json > verify.json   # 被测服务由 CI 先启动；exit 0 / 1 / 2
```

## 开发

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p requestbaby -- run collections
cargo run -p requestbaby-desktop -- <项目目录>
```

三个 crate：`requestbaby-core`（全部能力）、`requestbaby-cli`、`requestbaby-desktop`（GPUI）。每个能力只在 core 实现一次，两端都完整。
架构：[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)。

## License

MIT
