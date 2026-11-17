# requestbaby AI 原生调度协议

> requestbaby 定位：AI 编码工作流的**验证层**——大模型写代码/改接口，requestbaby 作为严格校验者，以全面的自动化接口测试保证产出质量。本文档定义 LLM 调度 requestbaby 的标准协议。
>
> **用例怎么写**见 [AI-GUIDE.md](AI-GUIDE.md)（生成指南）；本文只讲**怎么调度**。

## 核心闭环

```
        ┌─────────────────────── LLM（代码/用例生产者）───────────────────────┐
        │                                                                     │
        │  1. requestbaby schema --all          取四类配置契约（结构化约束） │
        │  2. 生成/修改 用例 JSON（受 schema 约束）                            │
        │                                                                     │
        ▼                                                                     │
   ┌─────────────┐   exit 2（配置错误）   ┌──────────────────────────────┐    │
   │  validate   │ ─────────────────────►│ 错误信息回喂 LLM 自修复      │◄───┘
   │  --json     │                       └──────────────────────────────┘
   └─────┬───────┘
         │ exit 0
         ▼
   ┌─────────────┐   打印 curl（不执行）→ LLM 人工审阅编译产物
   │  run        │
   │  --dry-run  │
   └─────┬───────┘
         │
         ▼
   ┌─────────────┐   exit 1（测试失败）→ --json 报告（含失败快照）回喂 LLM
   │  run        │ ────────────────────────────────────────────────────────► 修复循环
   │  --json -o  │   响应落盘 -o out/ 供 LLM 逐用例分析
   └─────────────┘
         │ exit 0
         ▼
      ✔ 通过（可作为代码质量门禁）
```

## 退出码语义（调度方必须区分）

| exit | 含义 | LLM 应对 |
|------|------|----------|
| 0 | 全部通过 | 完成/提交 |
| 1 | 测试失败（断言 false / 传输错误 / 脚本运行错误） | 读取 `--json` 报告的 tests.detail / response 快照，修复用例或被测代码 |
| 2 | 配置错误（schema 违规 / 路径不存在 / 环境文件损坏） | 读取 `validate --report json` 的 issues[].rule 定位修复 |

## 命令契约

### 1. 取配置契约（生成前必读）

```bash
requestbaby schema --all
```

输出 `{case, collection, env, project}` 四份 JSON Schema bundle。LLM 生成任何配置都必须通过该 schema（validate 会程序化校验，违反即 exit 2）。

### 2. 静态校验（零副作用，可高频调用）

```bash
requestbaby validate -e <env> --report json
```

（paths 缺省 `collections/`）

```json
{
  "ok": false,
  "errors": 2,
  "warnings": 1,
  "issues": [
    { "file": "collections/x.json", "level": "error", "rule": "schema",     "message": "...\"oops\": additional property is not allowed..." },
    { "file": "collections/x.json", "level": "error", "rule": "cases-file", "message": "数据文件不存在: data.csv" },
    { "file": "collections/x.json", "level": "warn",  "rule": "undefined-var", "message": "变量 {{token}} 无定义来源" }
  ]
}
```

规则集：`schema`（结构）/ `json-parse` / `serde-parse` / `script-syntax`（pre/post/tests）/ `undefined-var`（变量来源）/ `workflow-path` / `cases-file`。

### 3. 干跑审阅（编译产物可见，不执行）

```bash
requestbaby run <paths> --dry-run
```

每用例输出编译后的 curl 命令与最终 body（含 `{{var:number}}` 类型替换结果）。运行时捕获变量（`{{token}}` 来自前序 post）在 dry-run 中保留原样属预期。

### 4. 执行与结构化报告

```bash
requestbaby run -e <env> --report json -o out/
```

- 报告字段：`results[].{passed, error, tests[{expr, pass, detail}], response{status, headers, body}}`——失败用例自带响应快照（≤4KB，secret 自动脱敏）
- `-o out/` 每用例响应落盘 `out/**.resp.json`（完整 body），供 LLM 深度分析
- 失败细节语义：`求值为 false`（断言失败，业务问题）vs `执行错误: TypeError: ...（访问了 null/undefined 的 'x' 字段）`（配置问题，含修复提示）

### 5. 环境预检（调度前一次）

```bash
requestbaby doctor    # curl 版本/引擎/项目配置/环境文件
```

## 用例精简（生成后建议执行）

```bash
requestbaby fmt            # 重复 headers 自动上提集合级、字段规范排序、清理空字段
```

## 用例生成规范（给 LLM 的约束摘要）

1. **数字字段**必须用类型标注：`"taskId": "{{taskId:number}}"`（值替换为裸数字，int64 binding 安全）
2. **敏感值**用对象形态：`{"from": "env", "name": "TOKEN", "secret": true}`——不落盘、输出全脱敏
3. **断言**优先 `expectStatus` / `data.a.b`（res.json() 快照）/ `res.json('a.b')` 安全取值
4. **跨用例数据流**用 post 捕获：`vars.taskId = res.json('data.taskId')`；vars 保留原生类型（`env.x === 200` 数字比较正确）
5. **造数/清数**用 collection `setup`/`teardown`（内建 `http.get/post`），不依赖外部脚本
6. **多阶段编排**用 `workflows`（requestbaby.json），变量跨步骤自动传递

## CI 门禁模式

```yaml
# 代码合入门禁：LLM 生成的用例 + 接口回归
- run: requestbaby validate collections --report json || exit 2
- run: requestbaby run collections --report json --fail-fast > report.json
- run: requestbaby workflow e2e --report html
```

## 安全边界

- HTTP 引擎为内嵌 libcurl（静态链接单二进制），零外部进程依赖
- 脚本沙箱（QuickJS）无文件/网络自由权限，仅 `http.*` 受控请求 + `crypto` 纯函数
- `secret` 变量在所有输出层（curl 命令/日志/报告/快照/send 面板）强制脱敏
- `--tag` 过滤与 setup 副作用严格隔离：只读跑法不触发造数
