# requestbaby 架构设计

> CLI 中的 Postman：curl 为引擎、文件目录为集合、AI 原生 JSON 配置、面向项目级接口自动化测试。

## 1. 设计原则

1. **三层分工**：配置层（JSON + Schema，AI 生成/程序校验）、能力层（变量 + 脚本）、引擎层（curl 执行，不造 HTTP 轮子）
2. **AI 原生**：配置有 JSON Schema 约束 → `validate` 程序化校验 → `--dry-run` 编译出 curl 不执行，构成「AI 生成 → 校验 → 自修复」闭环
3. **一切皆文件**：目录树即集合层级，文件名序即执行序，Git 即协作
4. **可替换内核**：Transport / ScriptHost / Reporter 均为 trait，curl 引擎与 QuickJS 沙箱只是当前实现

## 2. 分层架构

```
┌───────────────────────────────────────────────────────────┐
│ CLI 层  clap: init / schema / validate / list / run       │
├───────────────────────────────────────────────────────────┤
│ 编排层  Runner                                            │
│   discover(路径) → plan(过滤) → execute(顺序) → report    │
├──────────────┬────────────────────────┬───────────────────┤
│ 配置层 config │ 能力层                  │ 引擎层 engine     │
│  Project     │  vars: 四层作用域替换   │  Transport(trait) │
│  Env         │  script: ScriptHost     │   └ CurlTransport │
│  Collection  │   (trait)               │    (spawn curl)   │
│  TestCase    │   └ QuickJsHost         │  Response 解析    │
│  + JSON      │   pre/post/tests        │                  │
│    Schema    │   + crypto 函数         │                  │
├──────────────┴────────────────────────┴───────────────────┤
│ 报告层 report: TerminalReporter / JsonReporter            │
└───────────────────────────────────────────────────────────┘
```

## 3. 目录规范（项目布局）

```
myapi/
├── requestbaby.json              # 项目配置：全局变量、默认环境、默认超时
├── envs/
│   ├── dev.json              # { "vars": {...} }
│   └── prod.json
└── collections/
    └── orders/               # 一级集合（目录即集合）
        ├── collection.json   # { "name", "vars", "tags" } 集合级配置
        ├── 01-login.json     # 用例（文件名序 = 执行序）
        ├── 02-create.json
        └── 90-refund/        # 二级集合（无限嵌套，collection.json 逐级继承）
```

变量作用域（低 → 高，高层覆盖低层）：

```
requestbaby.json vars  <  envs/<env>.json vars  <  各级 collection.json vars  <  运行时 vars（脚本写入）
```

## 4. 用例配置模型（docs/schema/case.schema.json 为准）

```json
{
  "name": "create-order",
  "tags": ["smoke", "order"],
  "skip": false,
  "request": {
    "method": "POST",
    "url": "{{baseUrl}}/orders",
    "headers": { "Authorization": "Bearer {{token}}" },
    "body": { "json": { "sku": "{{sku}}" } },
    "timeoutMs": 5000,
    "retry": 0,
    "insecure": false,
    "followRedirects": false
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

- `body` 形态：`{"json": <obj>}`（序列化 + 自动 Content-Type）/ `{"text": "<str>"}`
- JSON body 中 `{{x}}` 写在引号内 → 字符串替换；要求数值写在引号外：`{"qty": {{qty}}}`
- `pre`：请求前，可改写 `req.method/url/headers/body`，辅助 `req.setJson(obj)`
- `post`：响应后、断言前，写 `vars.*` 捕获传递
- `tests`：表达式数组，逐条求值，失败报告表达式与响应摘要

脚本沙箱注入：`req`（pre）/ `res`（post、tests）/ `vars`（读写）/ `env`（只读）/ `log()` / `crypto.{md5,sha256,hmacSha256,base64Encode,base64Decode}`

## 5. 核心数据流

```
TestCase ──load──► 合并变量作用域 ──pre 脚本──► ResolvedRequest
        ──编译──► curl argv + body 临时文件
        ──spawn curl──► stdout: %{json} 元数据 / -D: 响应头 / -o: 响应体
        ──解析──► Response ──post 脚本──► vars 回写
        ──tests 逐条求值──► CaseResult ──聚合──► Report
```

关键点：
- curl 参数数组经 `std::process::Command` 直接传递，**不经 shell**，天然免疫注入
- 响应元数据来自 `-w '%{json}'`（curl ≥ 7.75），避免混合输出解析
- curl 退出码非 0（连接拒绝/超时/DNS）作为传输错误报告，不进入断言阶段

## 6. 模块与扩展点

| 模块 | 职责 | 扩展点 |
|------|------|--------|
| `config` | Project/Env/Collection/TestCase 模型与加载（serde 严格模式，未知字段报错） | 新配置字段由 Schema 同步约束 |
| `vars` | 四层作用域栈、`{{}}` 替换 | 新变量来源（如 keychain） |
| `engine::Transport` | `execute(ResolvedRequest) -> Response` | 替换为 libcurl FFI / 内置 HTTP（二期协议扩展） |
| `script::ScriptHost` | `run_pre / run_post / eval_tests / compile_check` | 替换 JS 引擎；表达式方言 |
| `runner` | 目录发现、排序、过滤、fail-fast、顺序执行、并发（二期） | 调度策略 |
| `report::Reporter` | 终端美化 / JSON /（二期 junit、html） | 新报告格式 |
| `validate` | Schema 校验 + serde 严格解析 + lint（未定义变量、脚本预编译） | 新 lint 规则 |

## 7. 校验闭环（AI 原生机制）

```
AI 拿 docs/schema/case.schema.json 约束生成配置
        │
        ▼
requestbaby validate collections/     # 结构 + lint + 脚本语法
        │ 报错：文件/路径/规则，结构化输出
        ▼
requestbaby run collections/ --dry-run # 编译出 curl 命令打印，不执行
        │
        ▼
AI 依据报错自修复 → 循环直到通过
```

## 8. 技术选型

| 组件 | 选型 | 理由 |
|------|------|------|
| CLI | clap v4 derive | 子命令 + 补全 |
| 配置 | serde + serde_json（deny_unknown_fields） | 严格解析即第一层校验 |
| 脚本 | rquickjs（QuickJS） | 100% ES2020、静态链接、测试同学零学习成本 |
| 引擎 | `curl` crate（libcurl 官方 binding，static-curl 静态链接） | 单二进制内嵌 libcurl：零外部进程/动态库依赖，行为与系统 curl 一致（TLS/代理环境变量/HTTP2），dry-run 仍展示可复现 curl 命令 |
| 加密 | md-5 / sha2 / hmac / base64 | 脚本 crypto 函数的 Rust 后端 |

## 9. 演进记录（v0.2 架构）

- **执行模型**：串行（目录序+setup/teardown）→ 两阶段并行（`--parallel`，依赖用例自动串行前置）；副作用与 tag 过滤严格隔离
- **传输层**：`Transport` trait 收敛为 `execute(&ResolvedRequest) -> Response`；实现为 libcurl binding（static-curl，全内存执行无临时文件），hyper 实验引擎已移除
- **脚本层**：QuickJS Runtime 线程池化 + Context 每执行新建（隔离等价、开销降 90%）；`http.*` 受控同步请求；异常全透传（name/message/stack 行号修正）
- **变量层**：四层作用域 + `VarVal` 原生类型域（捕获的 number/bool 注入还原）；`{{var:number/bool}}` 类型标注、`{{%ENV}}` 进程环境、`{value,secret}`/`{from:env}` 对象形态（全输出层脱敏）
- **编排层**：`workflows`（requestbaby.json）多阶段编排，runtime vars 跨步骤传递；取代外部 shell 脚本
- **校验闭环（AI 原生）**：四类 JSON Schema 内置（validate 程序化校验防漂移）；退出码 0/1/2 语义；`validate --json` 结构化 issue（file/level/rule/message）；失败响应快照 + `-o` 响应落盘供 LLM 分析；`doctor` 环境预检——协议见 docs/AI-NATIVE.md
- **导入生态**：OpenAPI 3 / Swagger 2 / Postman v2.1 / curl 命令

## 10. 支持矩阵

- 协议：http/https
- 平台：macOS / Linux（Windows 未声明）
