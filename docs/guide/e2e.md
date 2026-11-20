# 配方：多步链路（E2E）

> `requestbaby guide e2e`

目标：教师布置 → 学生作答 → 教师查看报告，数据靠捕获一路传下去。

```text
collections/
├── 10-teacher/01-登录.json        capture: teacherToken
├── 10-teacher/02-布置作业.json    capture: taskId
├── 20-student/01-作答.json        用 {{taskId}}
└── 30-report/01-查看报告.json     用 {{taskId}}，expect 校验结果
```

```json case
{
  "name": "布置作业",
  "request": {
    "method": "POST",
    "url": "{{baseUrl}}/tasks",
    "headers": { "Authorization": "Bearer {{teacherToken}}" },
    "body": { "json": { "title": "期中练习", "classId": "{{classId:number}}" } }
  },
  "expect": { "status": 200, "body": { "code": 0, "data.taskId": { "type": "integer" } } },
  "capture": { "taskId": "data.taskId" }
}
```

```json case
{
  "name": "学生作答",
  "request": {
    "method": "POST",
    "url": "{{baseUrl}}/tasks/{{taskId}}/answers",
    "headers": { "Authorization": "Bearer {{studentToken}}" },
    "body": { "json": { "answers": ["A", "C"] } }
  },
  "expect": { "status": 200, "body": { "code": 0 } }
}
```

```json case
{
  "name": "教师查看报告",
  "request": { "method": "GET", "url": "{{baseUrl}}/tasks/{{taskId}}/report", "headers": { "Authorization": "Bearer {{teacherToken}}" } },
  "expect": {
    "status": 200,
    "body": {
      "data.taskId": "{{taskId}}",
      "data.submitted": 1,
      "data.students[*].score": { "gte": 0, "lte": 100 }
    }
  }
}
```

把顺序写成流程，`requestbaby workflow e2e` 一次跑完：

```json project
{
  "version": 1,
  "defaultEnv": "dev",
  "workflows": {
    "e2e": {
      "steps": [
        { "run": "collections/10-teacher" },
        { "run": "collections/20-student" },
        { "run": "collections/30-report" }
      ]
    }
  }
}
```

要点：
- 依赖只通过 `{{捕获变量}}` 表达，不写文件传数据
- 有副作用的用例打 `write` 标签：`requestbaby run --tag smoke` 只跑只读的
- 用例名写**预期**而不是操作：`03-重复提交被拒绝`

## 多端混合：API + Web + Android + Mac 桌面放进同一个流程

接口、网页、手机 App 的用例可以放在同一个流程里：运行时的变量是共用的，前一个用例捕获的值，后面任何端的用例都能用 `{{变量}}` 引用；整个流程得到一个结论，导出的报告是一个整体。

```text
collections/
├── 10-api-下单/01-下单.json          capture: orderNo
├── 20-android-支付/01-支付.json      tap 订单 {{orderNo}}，支付，capture: payNo
├── 30-web-后台审核/01-审核.json      fill {{orderNo}}，点「通过」
└── 40-api-查订单/01-状态.json        expect 状态已完成
```

```json project
{
  "version": 1,
  "defaultEnv": "dev",
  "workflows": {
    "mixed": {
      "steps": [
        { "run": "collections/10-api-下单" },
        { "run": "collections/20-android-支付" },
        { "run": "collections/30-web-后台审核" },
        { "run": "collections/40-api-查订单" }
      ]
    }
  }
}
```

- 数据怎么传：API 用 `capture` 从响应里取；Web / Android / Mac 桌面在任意一步上写 `capture`，从页面 / 界面的文字、输入框的值、属性、URL 里取（可加 `regex` 截取）。后面的用例在 URL、请求体、`fill` 的 `value` 里直接写 `{{orderNo}}`
- 顺序执行：界面用例与有依赖的用例一律串行，捕获一定先于引用；每个用例各自开浏览器 / 连设备，登录态不自动共享，各端各自登录（API 拿到的 token 可以捕获后填进页面）
- 一个整体的结果：`requestbaby workflow mixed` 一次跑完；`requestbaby export` 导出的 HTML 里，每个用例标着自己的端（API / Web / Android / Mac 桌面）；多端混合时顶部有「时间线」，把所有端的用例和每一步按真实先后排在一起，点一行跳到对应步骤；脚本 `log(...)` 的输出跟在用例里；失败现场（页面结构、控制台、网络请求、logcat、崩溃日志）直接嵌在报告里，单独发给别人也能看
- 要贴进聊天或给 AI：`requestbaby export -f text`，输出结论 + 时间线 + 失败详情的纯文本
- 异步联动（比如手机端操作后后台几秒才出数据）用 `waitFor` 等待，或调大那一步的 `timeoutMs`
