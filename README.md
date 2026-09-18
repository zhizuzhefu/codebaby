# requestbaby

> CLI 中的 Postman —— curl 为引擎、文件目录为集合、AI 原生 JSON 配置、面向项目级接口自动化测试。

```
      _                    _____
     / \   __ _  ___ _ __ |  ___|__  _ __ __ _  ___
    / _ \ / _` |/ _ \ '_ \| |_ / _ \| '__/ _` |/ _ \
   / ___ \ (_| |  __/ | | |  _| (_) | | | (_| |  __/
  /_/   \_\__, |\___|_| |_|_|  \___/|_|  \__, |\___|
          |___/                          |___/
```

## 为什么

| 痛点 | requestbaby 的答案 |
|------|----------------|
| Postman 集合是闭源 JSON 黑盒，无法 review | 纯 JSON 配置 + 目录树即集合，Git 原生管理 |
| 接口测试框架要写代码、搭工程 | 一个 JSON 一个用例，`pre/post/tests` 脚本内嵌 |
| 配置靠人手写易错 | **AI 原生**：`requestbaby schema` 导出 JSON Schema 约束生成，`validate` + `--dry-run` 程序化校验闭环 |
| HTTP 客户端轮子重 | curl 子进程执行，`-v` 打印完整 curl 命令可直接复现 |

## 安装

```bash
cargo install --path .
# 依赖：curl >= 7.75（支持 -w '%{json}'）
```

## 快速开始

```bash
requestbaby init myapi && cd myapi     # 生成脚手架
requestbaby validate collections       # 校验配置（结构/变量/脚本语法）
requestbaby run collections            # 跑整个集合
requestbaby run collections --tag smoke    # 标签过滤
requestbaby run collections -e prod        # 切换环境
requestbaby run collections --dry-run      # 打印编译出的 curl 命令，不执行
requestbaby run collections --json         # CI 报告，失败 exit 1
requestbaby list collections               # 列出用例
requestbaby schema                         # 导出用例 JSON Schema（喂给 AI）
```

## 用例长什么样

```json
{
  "name": "create-order",
  "tags": ["smoke", "order"],
  "request": {
    "method": "POST",
    "url": "{{baseUrl}}/orders",
    "headers": { "Authorization": "Bearer {{token}}" },
    "body": { "json": { "sku": "{{sku}}" } },
    "timeoutMs": 5000
  },
  "pre":  "req.headers['X-Ts'] = String(Math.floor(Date.now()/1000))",
  "post": "vars.orderId = res.json().data.id",
  "tests": [
    "res.status === 201",
    "res.json().data.sku === env.sku",
    "res.timeMs < 1000"
  ]
}
```

- `{{var}}`：变量替换（四层作用域：项目 < 环境 < 集合 < 运行时）
- `{{var:number}}` / `{{var:bool}}`：类型标注——JSON body 数字/布尔字段写 `"{{taskId:number}}"`，渲染后连引号替换为裸值，解决 int64 binding 拒绝字符串的问题
- `expectStatus`：状态码语法糖（`200` 或 `[200,409]`），免去每个用例写 `res.status === 200`
- 集合级 `headers`：collection.json 定义默认 headers 逐级继承，用例级同名覆盖——鉴权 API 只需配一次
- `pre`：请求前脚本，可改写 `req`（动态签名/时间戳）
- `post`：响应后脚本，`vars.x = ...` 捕获变量传给后续用例（登录→下单）
- `tests`：JS 表达式数组，逐条求值逐条报错；断言为 false 是正常失败，表达式抛错（如 null 链式取值）会指出具体字段名与修复提示

脚本沙箱可见：`req` / `res`（`status/headers/timeMs/json()/json(path)/text()`）/ `data`（= `res.json()` 快照，推荐 `data.a.b.c` 短写）/ `vars` / `env` / `log()` / `crypto.{md5,sha256,hmacSha256,base64Encode,base64Decode}`

> 脚本异常全透传：pre/post/tests 的运行时错误带 `name/message/stack`，stack 行号已修正为脚本内相对行号。

## 项目结构

```
myapi/
├── requestbaby.json               # 项目配置：全局变量/默认环境/默认超时
├── envs/
│   ├── dev.json               # { "vars": { "baseUrl": "..." } }
│   └── prod.json
└── collections/
    └── orders/                # 目录即集合（可无限嵌套）
        ├── collection.json    # 集合变量/标签，逐级继承
        ├── 01-login.json      # 文件名序 = 执行序
        └── 90-refund/
            └── 01-refund.json
```

## AI 原生工作流

```
1. requestbaby schema > case.schema.json          # 导出配置契约
2. 把 schema + 接口文档（OpenAPI/curl 示例/PRD）喂给 AI 生成用例 JSON
3. requestbaby validate collections               # 结构 + 变量引用 + 脚本语法
4. requestbaby run collections --dry-run          # 审阅编译出的 curl 命令
5. requestbaby run collections                    # 执行
   ↳ 报错结构化输出 → 回给 AI 自修复 → 循环
```

## CI 集成

```yaml
# GitHub Actions
- run: requestbaby run collections --fail-fast
# 失败 exit 1；--json 输出机器可读报告
```

## 更多

- 架构设计：[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
- 配置 Schema：[docs/schema/case.schema.json](docs/schema/case.schema.json)
- 可运行示例：[examples/demo-api](examples/demo-api)（基于 httpbin.org）

## Roadmap

- [ ] 数据驱动（`cases` 参数表）
- [ ] `--parallel` 并发执行
- [ ] JUnit XML / HTML 报告
- [ ] cookie jar 会话保持
- [ ] `requestbaby send` 交互式单请求调试

## License

MIT
