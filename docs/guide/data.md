# 配方：数据驱动

> `requestbaby guide data` · 桌面端：用例编辑器「高级」页「数据驱动」

一个用例 × 每行数据 = 一个实例，报告里显示为 `用例名 [label]`。行里的值是变量，优先级最高，只在本用例内可见。

```json case
{
  "name": "年龄校验",
  "request": {
    "method": "POST",
    "url": "{{baseUrl}}/users",
    "body": { "json": { "name": "{{name}}", "age": "{{age:number}}" } }
  },
  "expect": { "status": "{{status}}", "body": { "code": "{{code}}" } },
  "cases": [
    { "label": "未成年", "name": "小明", "age": 17, "status": 400, "code": 40001 },
    { "label": "边界 18", "name": "小红", "age": 18, "status": 200, "code": 0 },
    { "label": "负数", "name": "小刚", "age": -1, "status": 400, "code": 40002 }
  ]
}
```

- 行里直接写数字 / 布尔（`"age": 17`），断言、脚本里就是数字，不用 `Number()`
- 请求体的数字字段仍写 `"{{age:number}}"`
- 超过 2^53 的整数（雪花 ID）写成字符串，用 `{{id:number}}` 输出
- 外部数据：`"cases": {"file": "data/users.csv"}`（首行表头，值都是文本）或 `{"file": "data/users.json"}`（对象数组，保留类型）；路径相对用例文件

`expect.status` 写 `"{{status}}"` 即按行取状态码（行里也可以写 `"200,201"`）。
