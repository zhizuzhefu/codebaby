# 断言 expect

> `requestbaby guide expect` · 桌面端：用例编辑器「断言」页（状态码 / 响应体 / 响应头三张表，一行对应文件里一条）

## 结构

```json fragment
"expect": {
  "status": 200,
  "headers": { "Content-Type": { "contains": "json" } },
  "body": { "code": 0, "data.total": { "gte": 1 } }
}
```

三部分都可省略。按 状态码 → 响应头 → 响应体（书写顺序）逐条求值，每条都出现在报告里。

## 路径

| 写法 | 含义 |
|------|------|
| `code` | 响应体根上的 code |
| `data.user.name` | 逐级取字段 |
| `data.items[0].id` / `data.items.0.id` | 数组下标 |
| `data.items[*].price` | **每一个**元素都要满足；数组为空算失败（除非 `{"exists": false}`） |
| `$` | 整个响应体（非 JSON 响应可用 `{"$": {"contains": "ok"}}`） |

字段名含 `.` `[` `]` 时无法用路径表达，改用进阶的 `tests`。

## 期望值

- 字面量（数字 / 字符串 / 布尔 / null / 数组）= 相等；数字按数值比较（`1` 等于 `1.0`）；`"2"` 与 `2` 类型不同、不相等
- 对象且所有键都是运算符 = 运算符检查（全部满足才通过）；其余对象 = 整体相等

| 运算符 | 参数 | 通过条件 |
|--------|------|----------|
| `eq` / `ne` | 任意 | 等于 / 不等于（`eq` 用于比较含运算符名的对象） |
| `gt` `gte` `lt` `lte` | 数字 | 实际值是数字且满足比较 |
| `type` | `string` `number` `integer` `boolean` `null` `array` `object` | 类型相符（`integer` = 没有小数部分的数字） |
| `exists` | `true` / `false` | 字段存在（值为 null 也算存在）/ 必须不存在 |
| `len` | 数字或运算符对象 | 字符串字符数 / 数组元素数 / 对象键数 |
| `contains` | 任意 | 字符串含子串 / 数组含该元素 / 对象含该键 |
| `startsWith` `endsWith` | 字符串 | 字符串开头 / 结尾 |
| `matches` | 正则 | 字符串匹配（Rust regex 语法，JSON 里反斜杠写两个） |
| `oneOf` | 数组 | 等于其中之一 |
| `values` | 数组，或逗号分隔的文本 | 依次等于：`[*]` 收集到的全部值按顺序相等（路径指向数组时比较其元素） |

## 比较一整列：values

`[*]` 的含义是「每个元素都满足」；要比较收集到的整列（顺序、个数都算）用 `values`：

```json case
{
  "name": "页面按顺序返回",
  "request": { "method": "GET", "url": "{{baseUrl}}/reports/{{taskId}}" },
  "expect": {
    "status": 200,
    "body": {
      "data.files[*].pages[*].pageId": { "values": "{{expectedIds}}" },
      "data.files[*].pages[*].pageNo": { "values": [1, 1, 3] }
    }
  },
  "cases": [
    { "label": "选题作业", "taskId": 11, "expectedIds": "101,102,103" }
  ]
}
```

多层 `[*]` 会按顺序摊平（先第一个文件的全部页，再第二个文件……）。数据行里写逗号分隔的文本即可。

## 变量

期望值里可以写 `{{变量}}`：

```json case
{
  "name": "订单详情",
  "request": { "method": "GET", "url": "{{baseUrl}}/orders/{{orderId}}" },
  "expect": {
    "status": 200,
    "body": {
      "data.id": "{{orderId}}",
      "data.url": { "startsWith": "{{baseUrl}}/" },
      "data.items": { "len": "{{itemCount}}" },
      "data.amount": { "lte": "{{maxAmount}}" }
    }
  }
}
```

- 整个值恰好是 `"{{x}}"`：捕获的数字仍是数字；环境文件 / CSV 里的变量是文本，与数字按文本比较（`"3"` 与 `3` 相等），不需要转换
- 运算符参数（`gt`、`len`…）写 `"{{x}}"` 时按数字理解
- 要强制类型：`"{{x:number}}"` / `"{{x:bool}}"`

## 失败信息

```text
body.code = 0                          → 期望 0，实际 4030001
body.data.items[*].price > 0           → data.items[1].price：期望 > 0，实际 0（共 2 个）
body.data.token type string            → 字段不存在（期望 type string）
status = 200                           → 实际 500
```

## 照真实响应写

```bash
requestbaby send collections/orders/02-详情.json --suggest
```

输出每个字段的「等于当前值」与「类型」两种写法，以及可直接粘贴的 `capture`。值每次会变的字段（id、时间、token）用类型断言。桌面端响应面板「字段」页同一实现，点一下写进用例。

## 什么时候用进阶的 tests

expect 表达不了的才写 JS：跨字段计算、遍历比较、拼接后比较。

```json case
{
  "name": "列表按时间倒序",
  "request": { "method": "GET", "url": "{{baseUrl}}/orders" },
  "expect": { "status": 200, "body": { "data.items": { "len": { "gte": 2 } } } },
  "tests": ["data.data.items.every((x, i, a) => i === 0 || a[i - 1].createdAt >= x.createdAt)"]
}
```

`tests` 里 `data` 是整个响应体（所以这里是 `data.data.items`）。能逐字段写的检查放 expect：失败时才看得出是哪个字段。
已有的 JS 断言可以 `requestbaby fmt --fix` 自动改写成 expect（只改语义完全一致的）。
