# 命令与调度协议

> `requestbaby guide cli` · 给 AI / CI 调度 requestbaby 用：闭环、退出码、报告字段

被测服务由调度方自己启动；requestbaby 只发请求、断言、给结论。

## 闭环

```text
写 / 改用例 → validate（exit 2 → 读 issues 修配置）→ run --dry-run（审阅 curl）
→ run --report json（exit 1 → 读 tests[].detail 修用例或被测代码）→ run --failed → … → verify（exit 0 才算完成）
```

## 退出码

| exit | 含义 | 应对 |
|------|------|------|
| 0 | 全部通过 | 完成 |
| 1 | 用例失败（断言不成立 / 传输错误 / 脚本运行错误） | 读 `--report json` 的 `results[].tests[].detail` 与 `response` 快照 |
| 2 | 无法执行（配置错误 / 路径不存在 / 环境不存在） | 读 `validate --report json` 的 `issues[]` |

## 校验

```bash
requestbaby validate -e dev --report json
```

```json fragment
"ok": false, "errors": 1, "warnings": 2,
"issues": [
  { "file": "collections/a.json", "level": "error", "rule": "expect-matcher", "message": "expect.body.code：运算符对象里有未知运算符 lenn（…）" },
  { "file": "collections/b.json", "level": "warn", "rule": "prefer-expect", "message": "JS 断言可以写成声明式 expect：…（requestbaby fmt --fix · guide expect）", "fixable": true },
  { "file": "collections/c.json", "level": "warn", "rule": "undefined-var", "message": "变量 {{token}} 无定义来源" }
]
```

- error 规则：`schema` `json-parse` `serde-parse` `script-syntax` `expect-path` `expect-matcher` `expect-status-dup` `capture-source` `cases-file` `multipart-file` `workflow-path` `workflow-env` `env-missing`
- warn 规则（写法建议，消息里带 `guide <主题>`）：`prefer-expect` `prefer-capture` `legacy-expect-status` `long-js-test` `hardcoded-base-url` `redundant-number` `env-guard-script` `undefined-var` `workflow-empty`
- `"fixable": true` 的由 `requestbaby fmt --fix` 自动改写（语义不变）；`requestbaby fmt --fix --check` 只列出不改

## 运行与报告

```bash
requestbaby run --dry-run                 # 每个用例编译出的 curl（可直接粘贴执行），不发请求
requestbaby run -e dev --report json -o out/
requestbaby run --failed                  # 只重跑最近失败的，自动带上捕获变量的前置用例
requestbaby run --tag smoke --name 登录   # 过滤
requestbaby send collections/orders/01-创建订单.json --suggest   # 单发：完整响应 + 建议的 expect / capture
```

- `send` 用到前面用例捕获的变量（如 `token`）时，自动先运行捕获它的用例

- `results[].tests[]`：`{expr, pass, detail}`，如 `{"expr": "body.code = 0", "pass": false, "detail": "期望 0，实际 4030001"}`
- `results[].response`：响应快照（失败 ≤4000 字符、通过 ≤2000 字符，secret 脱敏）
- `compare`：与上次同环境运行对比 `{newFailures, fixed, stillFailing}`，先处理 `newFailures`
- `-o out/`：每个用例的完整响应落盘 `out/**.resp.json`

## 完整验证（交付结论）

```bash
requestbaby verify -e dev --report json
```

校验（有 error 即停）→ 全部用例 → 每个流程，一次给出结论。固定了其他环境的流程跳过（`notRun`）。报告的 `steps[]` 为各阶段，`blocked` 为无法完成的原因。

## 其他

```bash
requestbaby init                          # 项目骨架 + AGENTS.md / CLAUDE.md
requestbaby guide --init                  # 已有项目补 AGENTS.md / CLAUDE.md
requestbaby schema --all                  # 四类配置文件的 JSON Schema
requestbaby about                         # 版本、官网、源码、文档与反馈地址
requestbaby upgrade                       # 升级到最新版本（--check 只检查，有新版本时 exit 1）
requestbaby doctor                        # 环境自检：引擎 / 配置 / 环境文件 / baseUrl 可达 / 浏览器 / adb
requestbaby import openapi.json -o api/   # OpenAPI 3 / Swagger 2 / Postman v2.1 → 用例
requestbaby grpc list                     # gRPC 服务的全部方法；grpc describe <方法> 看请求消息骨架
requestbaby new case collections/chat 连接 --ws "{{wsBase}}/chat"   # 也有 --grpc <方法> / --web <地址> / --android <包名>
requestbaby web snapshot "{{webBase}}/login"   # 页面上的按钮 / 输入框与可直接粘贴的定位（android snapshot 读设备当前界面）
requestbaby android devices               # 已连接的 Android 设备（ANDROID_SERIAL=主机:端口 先自动连接远程设备）
requestbaby proxy                         # 代理：查看并检查；proxy set <地址> / on / off / unset；任意命令加 --proxy <地址> / --no-proxy 只影响这一次
requestbaby case cp collections/orders/02-创建.json 05-创建-vip    # 用例 / 集合：new / rename / cp / rm
requestbaby collection rename collections/orders 订单
requestbaby history                       # 运行历史；history rm / clear 清理
requestbaby export                        # 导出结果交给别人：全部用例的最新结果 → 验收报告-<项目>-<环境>.html；export <运行ID> 导出某一次；-f json / text（text = 结论 + 多端时间线 + 失败详情的纯文本）、-o 文件或目录、-e 环境
requestbaby fmt                           # 字段排序、清理空字段、公共请求头上提 collection.json
requestbaby open                          # 在桌面端打开当前项目
```

CI 门禁：`requestbaby verify -e ci --report json > verify.json`（被测服务由 CI 先启动）。
