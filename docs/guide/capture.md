# 捕获 capture

> `codebaby guide capture` · 桌面端：「断言」页底部「捕获为变量」表；响应面板「字段」页点「捕获…」

```json fragment
"capture": {
  "token": "data.token",
  "orderId": "data.items[0].id",
  "traceId": "header:X-Trace-Id"
}
```

- 来源：响应体路径（写法同 expect，不能含 `[*]`），或 `header:响应头名`
- 值保留原类型：数字仍是数字，`{{orderId}}` 渲染为 `1001`
- 取不到值 → 本用例失败：`capture token ← data.token → 响应中没有 data.token`
- 捕获在断言之前完成，本用例的 expect 里也能用 `{{token}}`

## 串联：前一个捕获，后一个引用

```json case
{
  "name": "创建订单",
  "request": { "method": "POST", "url": "{{baseUrl}}/orders", "body": { "json": { "sku": "A1", "qty": 2 } } },
  "expect": { "status": 201, "body": { "data.id": { "type": "integer" } } },
  "capture": { "orderId": "data.id" }
}
```

```json case
{
  "name": "查询刚创建的订单",
  "request": { "method": "GET", "url": "{{baseUrl}}/orders/{{orderId}}" },
  "expect": { "status": 200, "body": { "data.id": "{{orderId}}", "data.qty": 2 } }
}
```

- 用 `NN-` 文件名前缀保证先后（`01-创建订单.json`、`02-查询….json`）
- `codebaby run --failed` 只重跑失败的用例时，会自动带上捕获它所需变量的前置用例
- 捕获的变量在一次运行内有效；流程里跨步骤传递
- 请求体里的数字字段：`"orderId": "{{orderId:number}}"`

## 什么时候用进阶的 post 脚本

需要计算的值（拼接、过滤、取最大值）才写 `post`：

```json case
{
  "name": "取全部订单号",
  "request": { "method": "GET", "url": "{{baseUrl}}/orders" },
  "expect": { "status": 200 },
  "post": "vars.orderIds = data.data.items.map(x => x.id).join(',')"
}
```

`post` 里 `vars.x = data.a.b` 这种直接取值，`codebaby fmt --fix` 会改写成 capture。
