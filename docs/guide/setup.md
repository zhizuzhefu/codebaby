# 配方：造数清数

> `requestbaby guide setup` · 桌面端：集合设置「setup / teardown」

目标：集合运行前造好数据，运行后无论成败都清掉，用例可以无人值守重复跑。

`collection.json`：

```json collection
{
  "name": "订单",
  "setup": "const r = http.post(env.baseUrl + '/test/orders', { body: JSON.stringify({ sku: 'A1' }), headers: { 'Content-Type': 'application/json' } });\nvars.orderId = r.json('data.id')",
  "teardown": "http.request({ method: 'DELETE', url: env.baseUrl + '/test/orders/' + vars.orderId })"
}
```

目录下的用例直接用 `{{orderId}}`：

```json case
{
  "name": "订单可查询",
  "request": { "method": "GET", "url": "{{baseUrl}}/orders/{{orderId}}" },
  "expect": { "status": 200, "body": { "data.id": "{{orderId}}", "data.sku": "A1" } }
}
```

- `setup`：进入集合时（第一个要执行的用例之前）执行；`--tag` 过滤后集合里没有要跑的用例就不执行
- `teardown`：离开集合时执行，无论用例成败
- `--dry-run` 全部跳过，不会造数
- 脚本里可用 `http.get / post / request`（同步）、`vars`（写入的变量后续可用）、`env`（只读）、`crypto`、`log`；详见 `requestbaby guide scripts`
- 只能在某个环境造数（例如本地 fixture）：用流程固定环境，不要在 setup 里写 `if (env.baseUrl !== …) throw`（见 `requestbaby guide env`）
