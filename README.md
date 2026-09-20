# requestbaby

> CLI 侧的 Postman —— curl 为引擎、文件目录为集合、AI 原生 JSON 配置、面向项目级接口自动化测试。

```
 ____                             _         _         ____
|  _ \ ___  ___ _ __     _ __ ___| |_ _   _| | ___   / ___|__ _ _ __ ___   ___
| |_) / _ \/ __| '_ \   | '__/ _ \ __| | | | |/ _ \ | |   / _` | '_ ` _ \ / _ \
|  _ <  __/\__ \ |_) |  | | |  __/ |_| |_| | |  __/ | |__| (_| | | | | | |  __/
|_| \_\___||___/ .__/   |_|  \___|\__|\__,_|_|\___|  \____\__,_|_| |_| |_|\___|
               |_|
```

## 为什么

| 痛点 | requestbaby 的答案 |
|------|-------------------|
| Postman 集合是闭源 JSON 黑盒，无法 review | 纯 JSON 配置 + 目录树即集合，Git 原生管理 |
| 接口测试框架要写代码、搭工程 | 一个 JSON 一个用例，`pre/post/tests` 脚本内嵌 |
| 配置靠人手写易错 | **AI 原生**：`requestbaby schema` 导出 Schema 约束生成，`validate` + `--dry-run` 程序化校验闭环 |
| HTTP 客户端轮子重 | curl 子进程执行，`-v` 打印完整 curl 命令可直接复现 |
| 存量资产迁移难 | `import`：OpenAPI 3 / Swagger 2 / Postman v2.1 一键导入 |

## 安装

```bash
cargo install --path .     # 构建期依赖 cmake（编译内嵌 libcurl）
# 运行时零外部依赖：HTTP 引擎为 libcurl 官方 Rust binding（static-curl 静态链接进二进制），
# 不 spawn curl 进程、不动态链接系统 libcurl、无需任何运行时
```

## 快速开始

```bash
requestbaby init myapi && cd myapi   # 生成脚手架
requestbaby import openapi.json -o api/   # 从 spec 生成用例（OpenAPI3/Swagger2/Postman）
requestbaby validate collections     # 结构 + 变量引用 + 脚本语法校验
requestbaby run collections          # 跑整个集合
requestbaby send collections/01-login.json   # 单用例调试面板（状态/响应头/美化响应体/断言）
requestbaby watch collections        # 监听变更自动重跑
```

## 命令全景

```bash
requestbaby run <paths> [--tag smoke] [--name login] [-e prod]
                [--fail-fast] [--dry-run] [--json] [-v]
                [-o outdir]           # 响应落盘（AI 自修复素材）
                [--parallel 4]        # 独立用例并发
                [--report term|json|junit|html]
requestbaby workflow <name> [-e env]  # 命名工作流：多阶段编排，变量跨阶段自动传递
requestbaby send <case> [-e env]      # 交互式单发
requestbaby watch <paths> [...]       # 变更自动重跑
requestbaby import <spec> [-o dir]    # OpenAPI3/Swagger2/Postman 导入
requestbaby validate <paths> [-e env]
requestbaby list <paths>
requestbaby schema                    # 导出用例 JSON Schema（喂给 AI）
requestbaby init [dir]
```

## 项目结构

```
myapi/
├── requestbaby.json            # 项目配置：全局变量/默认环境/默认超时
├── envs/
│   ├── dev.json                # { "vars": {...} }
│   └── prod.json
└── collections/
    └── orders/                 # 目录即集合（无限嵌套，逐级继承）
        ├── collection.json     # vars + headers + tags + setup/teardown
        ├── 01-login.json       # 用例（文件名序 = 执行序）
        ├── 02-create.json      #   支持 cases 数据驱动
        └── data/users.csv
```

## 用例配置

```json
{
  "name": "create-order",
  "tags": ["smoke", "order"],
  "skip": false,
  "expectStatus": [200, 201],
  "request": {
    "method": "POST",
    "url": "{{baseUrl}}/orders",
    "headers": { "Authorization": "Bearer {{token}}" },
    "body": { "json": { "sku": "{{sku}}", "qty": "{{qty:number}}" } },
    "timeoutMs": 5000
  },
  "pre":  "req.headers['X-Ts'] = String(Math.floor(Date.now()/1000))",
  "post": "vars.orderId = res.json('data.id')",
  "tests": [
    "data.code === 0",
    "data.data.sku === env.sku",
    "res.timeMs < 1000"
  ],
  "cases": [
    { "label": "A1", "sku": "A1" },
    { "label": "B2", "sku": "B2" }
  ]
}
```

### 变量体系（四层，高覆盖低）

```
requestbaby.json vars  <  envs/<env>.json vars  <  collection.json vars  <  运行时 vars（脚本写入）
```

| 语法 | 说明 |
|------|------|
| `{{name}}` | 字符串替换 |
| `{{name:number}}` / `{{name:bool}}` | 类型标注：JSON body 值位置连引号替换为裸值（int64 binding 场景） |
| `{{%ENV_NAME}}` | 进程环境变量（CI 密钥不落盘） |

### 变量定义（对象形态）

```json
{
  "token":   { "value": "eyJ...", "secret": true },
  "apiKey":  { "from": "env", "name": "MY_KEY" }
}
```

- `secret: true`：所有输出（curl 命令/报告/日志/快照）自动脱敏为 `****`
- `from: "env"`：运行时读进程环境变量，缺失报错；默认视为 secret

### 脚本能力（QuickJS 沙箱，100% ES2020）

| 对象 | 能力 |
|------|------|
| `req` | pre 中改写 method/url/headers/body，`req.setJson(obj)` |
| `res` | `status/headers/timeMs/size/text()/json()/json('a.b.c')` 安全取值 |
| `data` | `res.json()` 快照，`data.a.b.c` 代替长链 |
| `vars` / `env` | 运行时变量读写 / 只读视图 |
| `http.request/get/post` | 同步发任意请求（setup 造数/清数、脚本内联调用） |
| `crypto` | md5 / sha256 / hmacSha256 / base64Encode / base64Decode |
| `log()` | 调试输出 |

脚本异常全透传：`name/message/stack`，stack 行号修正为脚本内相对行号；tests 区分「断言 false」（正常失败）与「表达式抛错」（配置问题，指出具体字段名与修复提示）。

### 集合级配置（collection.json，逐级继承）

```json
{
  "name": "orders",
  "tags": ["order"],
  "vars": { "sku": "A1" },
  "headers": { "authorization": "Bearer {{token}}" },
  "setup": "const r = http.post(env.baseUrl + '/tasks', {...}); vars.taskId = r.json().id",
  "teardown": "http.delete(env.baseUrl + '/tasks/' + vars.taskId)"
}
```

- headers/vars/tags：向下继承，用例级同名覆盖
- setup：进入集合执行（造数，可发请求）；teardown：离开执行（清数，无论成败）

### 工作流（多阶段编排）

E2E 链路不再依赖外部脚本串联：`requestbaby.json` 声明步骤，runtime vars 跨阶段自动传递。

```json
{
  "workflows": {
    "e2e": {
      "steps": [
        { "run": "collections/10-teacher", "tags": ["login"] },
        { "run": "collections/10-teacher", "tags": ["create", "verify"] },
        { "run": "collections/20-student" },
        { "run": "collections/10-teacher", "tags": ["poll", "send"] }
      ]
    }
  }
}
```

```bash
requestbaby workflow e2e -e test    # 步骤失败即中止；支持 --dry-run/--report
```

### 数据驱动

`"cases"` 内联行数组或 `{"file": "data.csv"}`（首行表头）；用例 × 每行 = 一个实例（`[label]` 标注），行变量优先级最高。

### 断言语法糖

- `expectStatus: 200` 或 `[200,409]`（幂等容忍）
- `data.a.b.c`（隐式 res.json() 根）
- `res.json('a.b.c')`（缺路径返回 undefined 不抛错）

## AI 原生工作流

```
1. requestbaby schema > case.schema.json        # 配置契约
2. AI 依 schema + 接口文档/OpenAPI 生成用例 JSON
3. requestbaby validate collections             # 结构 + 变量 + 脚本语法
4. requestbaby run collections --dry-run -v     # 审阅 curl 命令与最终 body
5. requestbaby run collections -o out/          # 执行 + 响应落盘
   ↳ 失败快照/脱敏报告/响应文件 → 喂回 AI 自修复 → 循环
```

## CI 集成

```yaml
- run: requestbaby run collections --fail-fast --report junit
# 或 --report html（artifact 挂载）；失败 exit 1
```

## 测试与示例

- 单元 + 集成测试：`cargo test`（内置 mini HTTP server 端到端）
- **全特性示范项目**：[examples/golden](examples/golden)——捕获链/数据驱动/setup/secret/workflow，进 CI 活验证（文档即契约）
- 可运行示例：[examples/demo-api](examples/demo-api)（httpbin）
- 用户项目：`projects/`（gitignore，明文密钥属用户资产）

## 文档

- **AI 用例生成指南**：[docs/AI-GUIDE.md](docs/AI-GUIDE.md)——字段全参考、变量系统、脚本沙箱 API、断言模式库、五种组织模式、生成工作流、错误修复对照表、自检清单
- **AI 调度协议**：[docs/AI-NATIVE.md](docs/AI-NATIVE.md)——闭环、退出码语义、CI 门禁
- 架构设计：[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)

## Roadmap

- [ ] native 引擎 feature（hyper 连接复用，大批量/压测档位）
- [ ] TUI 集合浏览器
- [ ] HAR / curl 命令导入
- [ ] 响应 schema 契约校验
- [ ] 执行历史与 `--compare last` 回归对比

## License

MIT
