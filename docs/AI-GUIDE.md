# requestbaby AI 用例生成指南

> 目标读者：大模型（LLM）。读完本文档 + `requestbaby schema --all` 的契约，你应当能：
> 1. 为任意 HTTP API 需求生成**正确的**项目级测试用例
> 2. 组织**复杂的多阶段自动化测试**（E2E 链路、数据驱动、回归集）
> 3. 通过 validate → run 的结构化反馈**自我修复**，直至全绿
>
> 配套调度协议见 [AI-NATIVE.md](AI-NATIVE.md)；完整可运行示范见 `examples/golden/`。

---

## 0. 五分钟最小闭环

```
1. requestbaby schema --all                     # 取四类配置契约
2. 写 collections/01-first.json（只需 method+url）
3. requestbaby validate --report json           # 结构校验（exit 2 = 配置错误）
4. requestbaby run --dry-run                    # 审阅编译出的 curl
5. requestbaby run --report json                # 执行（exit 1 = 测试失败，读报告自修复）
```

最小用例（其余全部可选、有默认）：

```json
{ "request": { "method": "GET", "url": "{{baseUrl}}/api/v1/users/me" } }
```

---

## 1. 项目结构与文件职责

```
<project>/
├── requestbaby.json      # 项目配置：全局 vars、defaultEnv、timeoutMs、workflows
├── envs/
│   └── test.json         # 环境：{ "vars": { "baseUrl": "...", "token": {...} } }
└── collections/          # 目录树即集合（可无限嵌套，逐级继承）
    ├── collection.json   # 集合级：vars / headers / tags / setup / teardown
    ├── 01-login.json     # 用例：文件名序 = 执行序（用 NN- 前缀控制顺序）
    └── 90-admin/…
```

**继承链**（同名后者覆盖前者）：项目 vars < 环境 vars < 集合 vars < 用例 `cases` 行变量 < 运行时 vars（脚本捕获）。

**生成规则**：
- 同域接口放同一目录；目录放 `collection.json` 声明公共 headers（鉴权头只写一次）
- 执行顺序敏感的用例（登录 → 操作）用 `NN-` 前缀保证顺序，并让后者用 `{{捕获变量}}` 显式依赖前者
- 副作用分级打 tag：只读 `smoke` / 写入 `write` / 发通知 `send` —— `--tag smoke` 保证零副作用

---

## 2. 用例字段参考（TestCase）

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `name` | string | 文件名去扩展 | 展示名，`--name` 按它+文件名过滤 |
| `tags` | string[] | [] | 与集合 tags 合并；`--tag a,b` 过滤 |
| `skip` | bool | false | 保留计划暂不执行 |
| `expectStatus` | int \| int[] | 无 | 状态码断言糖：`200` 或 `[200,409]`（幂等容忍） |
| `request` | object | **必填** | 见下表 |
| `pre` | string | "" | 前置 JS：改写 `req`（签名/时间戳/动态 body） |
| `post` | string | "" | 后置 JS：`vars.x = ...` 捕获传递 |
| `tests` | string[] | [] | 断言表达式数组，逐条求值逐条报错 |
| `cases` | array \| {file} | 无 | 数据驱动：行数组或 `{"file":"data.csv"}` |

**request 字段**：

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `method` | string | 必填 | GET/POST/PUT/PATCH/DELETE/HEAD/OPTIONS |
| `url` | string | 必填 | 支持 `{{var}}`；query 直接写在 URL |
| `headers` | object | 集合继承 | key 不区分大小写合并；值支持 `{{var}}` |
| `body` | object | 无 | `{"json": <obj>}`（自动 Content-Type）或 `{"text": "..."}` |
| `timeoutMs` | int | 项目 timeoutMs | 单请求超时 |
| `retry` | int | 0 | 传输层重试（curl --retry） |
| `insecure` | bool | false | 跳过证书校验 |
| `followRedirects` | bool | false | 跟随 3xx |

---

## 3. 变量系统（生成用例最容易犯错的地方）

| 语法 | 用途 | 示例 |
|------|------|------|
| `{{name}}` | 字符串替换 | `"Authorization": "Bearer {{token}}"` |
| `{{name:number}}` | **JSON 数字字段必用**：渲染连引号变裸数字 | `"taskId": "{{taskId:number}}"` |
| `{{name:bool}}` | 布尔裸值 | `"force": "{{force:bool}}"` |
| `{{%ENV_NAME}}` | 读进程环境变量（CI 密钥） | `"?key={{%API_KEY}}"` |

**变量定义**（requestbaby.json / envs / collection.json 的 vars）三种形态：

```json
{
  "baseUrl": "https://api.test.example",
  "orgId": 11,
  "token": { "value": "eyJ...", "secret": true },
  "apiKey": { "from": "env", "name": "MY_KEY" }
}
```

- 标量自动字符串化；`secret: true` 的值在**所有输出**（curl 命令/日志/报告/快照）自动脱敏为 `****`
- `from: "env"` 运行时读环境变量，默认视为 secret —— **密钥永不落盘**

**运行时捕获**（post 脚本）：`vars.x = res.json('data.id')` —— **保留原生类型**，后续 `env.x === 200` 数字比较正确。`{{x}}` 渲染时数字自动转字符串。

> 高频错误：body 数字字段写 `"{{taskId}}"`（缺 `:number`）→ 服务端 int64 binding 报错。**数字一律加 `:number`**。

---

## 4. 脚本沙箱 API（QuickJS，100% ES2020）

pre / post / setup / teardown / tests 共用同一沙箱。**无文件与网络自由权限**。

### req（仅 pre 可写）

```js
req.method            // "POST"
req.url               // 已做变量替换
req.headers['X-Ts'] = String(Math.floor(Date.now()/1000))   // 改写头
req.setJson({ sku: env.sku, qty: Number(env.qty) })          // 结构化改写 body
req.body = JSON.stringify({...})                             // 或直接写字符串
```

### res（post / tests 可读）

```js
res.status             // 200
res.timeMs             // 412（耗时断言：res.timeMs < 1000）
res.size
res.headers['content-type']   // 小写 key
res.text()             // body 原文
res.json()             // 解析后的对象
res.json('a.b.0.c')    // 安全取值：路径缺失返回 undefined 不抛错
```

### data（= res.json() 快照，**断言首选**）

```js
data.code === 0
data.data.items.length > 0        // 链式比 res.json() 短
```

### vars / env

```js
vars.orderId = res.json('data.id')   // 捕获（原生类型保留）
env.baseUrl                           // 只读视图（含全部层合并 + 捕获值）
```

### http（pre/post/setup/teardown 可用；tests 不可）

```js
const r = http.get(url, { headers: {...}, timeoutMs: 5000 })
const r = http.post(url, { body: '{"a":1}', headers: {...} })
r.status; r.body; r.headers; r.json(); r.json('data.id')
// 造数/清数/脚本内联调用（同步）
```

### crypto / log

```js
crypto.md5(s)  crypto.sha256(s)  crypto.hmacSha256(key, msg)
crypto.base64Encode(s)  crypto.base64Decode(s)
log('任何对象')   // -v 时输出（secret 自动脱敏）
```

---

## 5. 断言模式库（tests 写法速查）

```js
// 状态：优先用 expectStatus 字段；复杂场景才写表达式
[ "res.status === 200" ]

// 业务码（国内 API 惯例 code=0）
[ "data.code === 0", "data.message === 'success'" ]

// 字段存在性与类型
[ "data.data.id !== undefined", "typeof data.data.name === 'string'" ]

// 数组
[ "Array.isArray(data.data.items)", "data.data.items.length > 0" ]

// 数值范围
[ "data.data.totalCount >= 0", "res.timeMs < 1000" ]

// 与变量联动（捕获值/环境值/数据行值）
[ "data.data.sku === env.sku", "data.data.id === Number(env.orderId)" ]

// 安全模式（防 null 链式崩溃，推荐）
[ "res.json('a.b.c') !== undefined" ]        // 存在性
[ "data.a && data.a.b === 1" ]               // 短路

// 幂等容忍
{ "expectStatus": [200, 409] }
```

**两种失败的语义**（自修复时必须区分）：
- `求值为 false` → 断言失败：**业务问题**，对照 detail 里的响应摘要修用例或修代码
- `执行错误: TypeError: cannot read property 'x' of null（访问了 null/undefined 的 'x' 字段）` → **配置问题**：路径写错，改用 `res.json('a.b')` 或短路写法

---

## 6. 组织模式（按需求类型选择）

### A. 单接口测试（冒烟/回归）

一个文件一个接口；公共鉴权放 `collection.json` headers。

### B. E2E 链路（多角色/多步骤）

**数据流用捕获链，不用文件传递**：

```
01-login.json      post: vars.token = res.json('data.token')
02-create.json     post: vars.taskId = res.json('data.taskId')
03-do.json         url: ".../tasks/{{taskId}}"  headers: Bearer {{token}}
```

跨阶段（教师端→学生端→教师端）用 **workflow**（requestbaby.json）：

```json
{ "workflows": { "e2e": { "steps": [
  { "run": "collections/10-teacher", "tags": ["login"] },
  { "run": "collections/20-student" },
  { "run": "collections/10-teacher", "tags": ["poll"] }
] } } }
```

`requestbaby workflow e2e` 一次跑完，变量跨步骤自动传递，步骤失败即中止。

### C. 造数/清数（无人值守）

collection.json：

```json
{
  "setup": "const r = http.post(env.baseUrl + '/tasks', { headers: env._h }); vars.taskId = r.json('data.id')",
  "teardown": "http.delete(env.baseUrl + '/tasks/' + vars.taskId)"
}
```

- setup 仅在该集合有存活用例时执行（`--tag` 过滤不误触）
- teardown 无论成败必执行；dry-run 全部跳过

### D. 数据驱动（边界覆盖）

```json
{
  "request": { "method": "POST", "url": "{{baseUrl}}/users", "body": { "json": { "age": "{{age:number}}" } } },
  "cases": [
    { "label": "未成年", "age": "17" },
    { "label": "边界18", "age": "18" },
    { "label": "非法", "age": "-1" }
  ],
  "tests": ["data.code === 0"]
}
```

报告按 `用例名 [label]` 分实例；行变量优先级最高。CSV：首行表头，`{"file": "data.csv"}`。

### E. 回归集

- tag 分层：`smoke`（只读，每次跑）/ `reg`（全量）/ `send`（发通知，人工触发）
- 命名即文档：`NN-场景-预期[bug:背景]` 如 `05-重练报告-summary字段完整[bug:缺未完成薄弱人数]`
- 用例名描述**预期**而非操作

---

## 7. 生成工作流（需求 → 全绿）

```
① 解析需求 → 列接口清单（method/path/鉴权/依赖/副作用）
② 生成项目骨架：requestbaby.json + envs/<env>.json（baseUrl + 鉴权 secret 变量）
   + collection.json（公共 headers + tags）
③ 逐接口生成用例：正例（expectStatus + data.code + 核心字段 3 条以内断言）
   + 负例（参数缺失/越界/无权限）+ 幂等（[200,409]）
   依赖链用捕获；边界用 cases 数据驱动
④ requestbaby fmt                                  # 形态收敛
⑤ requestbaby validate --report json   exit 2 →   # 读 issues[].rule 修配置
⑥ requestbaby run --dry-run            # 审阅 curl 与最终 body（数字是否裸值）
⑦ requestbaby run --report json        exit 1 →   # 读 tests[].detail + response 快照修
⑧ requestbaby workflow <name>          # E2E 整链验证
⑨ 循环 ⑤-⑧ 至全绿；提交
```

---

## 8. 错误 → 修复对照表（自修复钥匙）

| 报错特征 | 根因 | 修复 |
|----------|------|------|
| `占位符未替换: 变量 X 未定义` | vars 无来源 | 在 envs/collection 定义，或前序用例 post 捕获 |
| `占位符未替换: 环境变量 X 未设置` | `{{%X}}` 无进程环境 | export 或改用 envs 定义 |
| `执行错误: TypeError ... of null（访问了 null 的 'x' 字段）` | 断言路径错 | `res.json('a.b')` / `data.a && ...` |
| `pre/post 脚本失败: TypeError ... at (eval_script:N:C)` | 脚本第 N 行第 C 列抛错 | 按行号修复 |
| `求值为 false` + detail 响应摘要 | 断言与实际不符 | 修断言或修被测代码 |
| `传输错误: curl exit 7` | 连不上 | baseUrl/网络/服务未起 |
| `传输错误: curl exit 28` | 超时 | timeoutMs 调大或服务慢查询 |
| 服务端报"参数类型错误" | 数字字段缺 `:number` | `"id": "{{id:number}}"` |
| 401/403 | token 失效或角色不符 | 更新 envs token；换有权限账号 |
| validate `rule: undefined-var`（warn） | 变量无静态来源 | 确认捕获链或忽略 |

---

## 9. 生成后自检清单

- [ ] `validate --report json` → `"ok": true`
- [ ] `fmt --check` → 已是精简形态
- [ ] `run --dry-run` → curl 命令里数字字段是裸值（无引号）
- [ ] secret 只出现在对象形态定义处，其余输出全是 `****`
- [ ] 每个依赖用例的 `{{捕获变量}}` 在前序 post 中有对应 `vars.x = ...`
- [ ] 写操作用例已打 `write`/`send` tag，`--tag smoke` 零副作用
- [ ] E2E 有 workflow 定义且 `workflow <name>` 全绿

---

## 10. 完整示范

`examples/golden/` 覆盖本文全部特性（捕获链/类型标注/数据驱动/setup/secret/workflow/负例），其集成测试保证文档描述与实现**永久一致**——文档即契约。
