# 流程 workflows

> `requestbaby guide flow` · 桌面端：侧栏「流程」，流程编辑器

流程 = 按顺序跑几个集合 / 用例，捕获的变量跨步骤传递，某一步失败即停止。写在 `requestbaby.json`：

```json project
{
  "version": 1,
  "defaultEnv": "dev",
  "workflows": {
    "e2e": {
      "name": "下单到发货",
      "steps": [
        { "run": "collections/auth" },
        { "run": "collections/orders", "tags": ["create"] },
        { "name": "发货", "run": "collections/ship" }
      ]
    }
  }
}
```

- `run`：相对项目根的目录或用例文件；`tags` / `nameFilter` 只跑其中一部分
- `env`：固定运行环境（见 `requestbaby guide env`）
- `needs`：依赖的流程（见下一节「依赖与登录态」）
- `as`（步骤上）：以哪个用户运行（见下一节「切换用户」）

## 命令（与桌面端流程编辑器同一实现）

```bash
requestbaby workflow e2e                      # 运行
requestbaby workflow list
requestbaby workflow new e2e --step collections/auth --step collections/orders
requestbaby workflow step add e2e collections/ship --name 发货
requestbaby workflow step mv e2e 3 1          # 第 3 步移到第 1 位（从 1 开始数）
requestbaby workflow show e2e
requestbaby workflow rename e2e order-flow
requestbaby workflow env order-flow local     # 固定环境
requestbaby workflow rm order-flow
```

## 依赖与登录态：登录只写一次

每个业务场景都要先登录，不必每个流程都从零登录：把登录做成一个流程，业务流程用 `needs` 依赖它。

```json project
{
  "version": 1,
  "defaultEnv": "dev",
  "workflows": {
    "login": { "name": "登录", "steps": [{ "run": "collections/00-auth" }] },
    "order": { "needs": ["login"], "steps": [{ "run": "collections/10-order" }] },
    "report": { "needs": ["login"], "steps": [{ "run": "collections/20-report" }] }
  }
}
```

- 运行 `requestbaby workflow order`，会先运行它依赖的 `login`，再运行自己的步骤；依赖可以再依赖别的流程（成环会报错），多个依赖按先后顺序、各跑一次
- **登录态沿用到本流程所有步骤**：Cookie、网页存储（localStorage / sessionStorage，前端应用的 token 常放在这里）、捕获的变量。后面的 Web 用例打开页面时已经是登录状态，不用再点一遍登录
- `requestbaby verify` 里依赖相同的流程（上面的 `order` 与 `report`）共用同一次登录；没有 `needs` 的流程始终从零开始，不会带上别的流程的登录态。依赖不同的流程各自跑自己的依赖
- 只想跑本流程自己的步骤（登录态由别处提供）：`requestbaby workflow order --no-needs`
- 只有依赖、没有自己步骤的流程也可以，相当于「准备好某个状态」
- 依赖的流程要在同一个环境里运行：依赖的流程固定了环境（`env`），本流程也要固定到同一个
- 桌面端：流程编辑器里「依赖的流程」点「添加依赖」；运行时进度条把依赖的步骤排在前面（`登录 › 步骤名`）

```bash
requestbaby workflow new order --needs login --step collections/10-order
requestbaby workflow needs order                 # 查看依赖
requestbaby workflow needs order add report-base # 增加依赖
requestbaby workflow needs order rm login        # 去掉依赖
```

重命名流程时，别的流程里对它的依赖会一起改名；被别的流程依赖的流程不能直接删除。

### 切换用户

老师布置、学生作答：同一个流程里要用不同的账号。步骤写 `as`，每个名字是一个独立的用户，各有一份登录态（Cookie、网页存储），来回切换互不影响；捕获的变量（比如 `taskId`）仍然所有用户共享。不写 `as` 就是默认用户。

```json project
{
  "version": 1,
  "defaultEnv": "dev",
  "workflows": {
    "teacher-login": { "steps": [{ "run": "collections/00-auth/teacher" }] },
    "student-login": { "steps": [{ "run": "collections/00-auth/student", "as": "student" }] },
    "homework": {
      "needs": ["teacher-login", "student-login"],
      "steps": [
        { "name": "老师布置", "run": "collections/10-assign" },
        { "name": "学生作答", "run": "collections/20-answer", "as": "student" },
        { "name": "老师查看报告", "run": "collections/30-report" }
      ]
    }
  }
}
```

命令行：`requestbaby workflow step add homework collections/20-answer --as student`。Android / Mac 桌面用例的登录态保存在手机 / 应用里，`as` 对它们没有作用（需要换账号就在用例里退出再登录）。

`requestbaby verify` 会依次跑：校验 → 全部用例 → 每个流程，一次给出结论。
