# requestbaby 速查卡

> 按场景深入：`requestbaby guide <主题>`，主题见文末。

## 5 个概念

| 概念 | 是什么 | 在哪 |
|------|--------|------|
| 用例 | 一个请求 + 它应得的响应 | `collections/**/NN-名称.json`（名称可用中文），文件名序 = 执行序；其他协议把 `request` 换成 `grpc` / `ws` / `web` / `android` / `desktop` |
| 集合 | 一个目录；公共请求头、标签 | 目录里的 `collection.json`（可选） |
| 环境 | 一组变量：`baseUrl`、账号 | `envs/<环境>.json`；`baseUrl` 只写到端口，`/api/…` 写在用例 URL |
| 断言 | 响应应该是什么样 | 用例的 `expect` |
| 流程 | 按顺序跑几个集合 / 用例；API / Web / Android / Mac 可混在一个流程 | `requestbaby.json` 的 `workflows` |

## 一个完整的用例

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
      "data.token": { "type": "string" },
      "data.roles": { "contains": "admin" }
    }
  },
  "capture": { "token": "data.token" }
}
```

后面的用例直接用捕获的变量（URL、请求头、断言里都写 `{{名字}}`）：`"url": "{{baseUrl}}/orders/{{orderId}}"`、`"data.id": "{{orderId}}"`。

token 这类每个用例都要带的请求头，在目录的 `collection.json` 写一次：

```json collection
{ "headers": { "Authorization": "Bearer {{token}}" } }
```

用例里同名请求头覆盖它；验证未登录被拒：该头写 `{ "value": "", "disabled": true }`，`expect.status` 写 401。

## expect：写响应应该是什么样

- `status`：`200` 或 `[200, 201]`；`headers`：响应头名（不分大小写）→ 期望值
- `body`：路径 → 期望值。路径从响应体根写起：`code`、`data.user.name`、`data.items[0].id`；`[*]` = 每个元素都要满足；`$` = 整个响应体
- 期望值写字面量 = 相等：`null` = 值是 null（`{"exists": false}` 才是字段不存在）；数字按数值精确比较、没有容差，算出来的小数用 `{"gte": 39.79, "lte": 39.81}`
- 要别的条件写运算符对象，多个运算符同时满足：

| 运算符 | 含义 | 例 |
|--------|------|----|
| `ne` | 不等于 | `{"ne": ""}` |
| `gt` `gte` `lt` `lte` | 数字比较 | `{"gte": 1, "lte": 100}` |
| `type` | string / number / integer / boolean / null / array / object | `{"type": "integer"}` |
| `exists` | 字段存在（`false` = 必须不存在） | `{"exists": false}` |
| `len` | 长度（数字或运算符） | `{"len": 3}`、`{"len": {"gt": 0}}` |
| `contains` `startsWith` `endsWith` | 包含 / 开头 / 结尾 | `{"startsWith": "https://"}` |
| `matches` | 正则 | `{"matches": "^\\d{6}$"}` |
| `oneOf` | 属于其一 | `{"oneOf": ["paid", "shipped"]}` |

## capture：从响应取值给后面的用例

`"capture": { "变量名": "响应体路径" }`，响应头写 `"header:X-Trace-Id"`。取不到值时本用例失败，并写明缺哪个字段。
一次运行里按执行顺序传给后面所有用例（跨文件、跨目录，不需要流程）。

## 变量：日常只需要两层

- **环境变量**：`envs/<环境>.json`，用 `requestbaby vars set baseUrl http://localhost:8080 -e dev` 写入；密钥加 `--secret`，只在本机的值加 `--local dev`
- **捕获变量**：上面的 `capture`（流程里跨步骤也传）

用法：`{{name}}`，URL 里直接写。JSON 请求体里的数字字段写 `"id": "{{id:number}}"`（输出裸数字）。断言里与数字比较不用转换（环境里的 `"42"` 等于 42）。

## Web / Android / Mac 与多端混合

`web` / `android` / `desktop`（Mac 应用）用例写步骤（点击、填写、断言文字），每步自动截图；先用 `web snapshot <url>` / `android snapshot` / `desktop snapshot <应用>` 看元素与定位（`guide web` / `android` / `desktop`）。各端用例可放进同一个流程，`capture` 的值互通。每个场景都要先登录：登录做成一个流程，其他流程 `"needs": ["login"]` 依赖它（登录态沿用，不重复登录）；步骤 `"as": "student"` 切换用户（`guide flow`）。

## 闭环命令

顺序：`validate` → `run` → `verify`；`-e <环境>` 选环境，缺省用项目默认环境。

```bash
requestbaby new case collections/orders 创建订单 -m post   # 新建用例（自动编号）
requestbaby new case collections/orders --curl "curl …"   # 已有 curl 直接转用例
requestbaby send collections/orders/01-创建订单.json --suggest   # 看真实响应 + 照它生成 expect / capture
requestbaby validate --report json   # exit 2 = 配置写错；warn 是写法建议
requestbaby run --report json        # exit 1 = 断言失败：读 results[].tests[].detail 修
requestbaby run --failed             # 只重跑失败的（自动带上它依赖的前置用例）
requestbaby verify -e dev            # 最终结论：校验 → 全部用例 → 全部流程；0 通过 / 1 失败 / 2 无法完成
```

## 规矩（写偏了 validate 会提示；多数 `requestbaby fmt --fix` 能自动改写）

1. 断言写 `expect`；只有跨字段计算、遍历比较才用进阶的 `tests`（JS）
2. 取值写 `capture`，不写 `post` 脚本
3. URL 写 `{{baseUrl}}/路径`，域名只放在环境里
4. 有先后依赖的用 `NN-` 排序，靠 capture 传值
5. 只能在某个环境跑的，用流程固定环境（`requestbaby workflow env <流程> <环境>`），不写环境守卫脚本
6. 交付以 `requestbaby verify` 的 exit 0 为准，不另写脚本串联

## 主题（`requestbaby guide <主题>`）

`expect` 断言 · `capture` 捕获与串联 · `vars` 变量进阶 · `flow` 流程 · `auth` 鉴权登录 · `e2e` 多步 · `data` 数据驱动 · `setup` 造数清数 · `env` 环境 · `grpc` gRPC · `ws` WebSocket · `web` 网页 · `android` 安卓 · `desktop` Mac 桌面 · `ocr` 文字识别 · `control` 循环回调 · `proxy` 代理 · `scripts` 脚本 · `cli` 命令 · `ref` 全部字段 · `all` 全部
