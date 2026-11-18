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
