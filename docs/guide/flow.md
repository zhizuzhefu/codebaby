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

`requestbaby verify` 会依次跑：校验 → 全部用例 → 每个流程，一次给出结论。
