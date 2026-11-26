# 进阶：JS 脚本

> `codebaby guide scripts` · 桌面端：用例编辑器「脚本（进阶）」页

断言用 `expect`、取值用 `capture` 覆盖不了时才写脚本。沙箱是 QuickJS（ES2020），没有文件与网络的自由访问。

| 字段 | 何时执行 | 典型用途 |
|------|----------|----------|
| `pre` | 请求发出前 | 签名、时间戳、动态请求体 |
| `post` | 响应后、断言前（capture 之后） | 计算后的值写入 `vars` |
| `tests` | 断言阶段（expect 之后） | 每条一个 JS 表达式，真值为通过 |
| 集合 `setup` / `teardown` | 进入 / 离开集合 | 造数清数（`codebaby guide setup`） |

```json case
{
  "name": "签名请求",
  "request": { "method": "POST", "url": "{{baseUrl}}/open/orders", "body": { "json": { "sku": "A1" } } },
  "pre": "const ts = String(Math.floor(Date.now() / 1000));\nreq.headers['X-Ts'] = ts;\nreq.headers['X-Sign'] = crypto.hmacSha256(env.appSecret, ts + req.body)",
  "expect": { "status": 200 },
  "post": "vars.skuList = data.data.items.map(x => x.sku).join(',')",
  "tests": ["data.data.items.every(x => x.qty > 0 && x.price * x.qty === x.amount)"]
}
```

## API

```text
req.method / req.url / req.headers / req.body     pre 可改；req.setJson(obj) 改写 JSON 请求体
res.status / res.timeMs / res.size                post、tests 可读
res.headers['content-type']                       响应头（小写名）
res.text() / res.json() / res.json('a.b.0.c')    原文 / 解析 / 安全取值（缺失返回 undefined）
data                                              = res.json()（整个响应体）
vars.x = …                                        写入变量（保留原类型），后续用例 {{x}}
env.x                                             只读：全部层合并后的变量值
http.get(url, opts) / http.post(url, opts) / http.request({method, url, headers, body})   pre/post/setup/teardown 可用
crypto.md5 / sha256 / hmacSha256(key, msg) / base64Encode / base64Decode
log(任何值)                                       -v 时输出（secret 自动脱敏）
```

## 失败语义

- `求值为 false`：断言不成立 —— 业务问题，对照报告里的响应快照
- `执行错误: TypeError … of null（访问了 null/undefined 的 'x' 字段）`：路径写错 —— 改用 `res.json('a.b')`，或改写成 expect（字段不存在会直接说明）

`tests` / `post` 里能逐字段表达的部分，`codebaby fmt --fix` 会改写成 expect / capture；一条断言是一大段程序时 validate 提示 `long-js-test`。
