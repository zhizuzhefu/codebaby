# 变量进阶

> `requestbaby guide vars` · 日常只需要环境变量 + 捕获变量；本页是其余层级与类型细节

## 语法

| 写法 | 用途 | 例 |
|------|------|----|
| `{{name}}` | 文本替换（URL、请求头、请求体、期望值） | `"Bearer {{token}}"` |
| `{{name:number}}` | JSON 数字字段：连引号替换为裸数字 | `"qty": "{{qty:number}}"` |
| `{{name:bool}}` | JSON 布尔字段 | `"force": "{{force:bool}}"` |
| `{{%ENV_NAME}}` | 读进程环境变量（CI 密钥） | `"key={{%API_KEY}}"` |

## 层级（同名时右边覆盖左边）

项目 `requestbaby.json` < 环境 `envs/<环境>.json` < 本机覆盖 `envs/<环境>.local.json` < 集合 `collection.json`（浅 → 深）< 数据驱动行 < 捕获

## 定义形态

```json fragment
"vars": {
  "baseUrl": "https://api.test.example",
  "orgId": 11,
  "token": { "value": "eyJ…", "secret": true },
  "apiKey": { "from": "env", "name": "MY_KEY" }
}
```

- 环境文件里的标量都当文本（`11` → `"11"`）；数据驱动行与捕获保留原类型
- `secret: true`：curl 命令、日志、报告、快照里都显示 `****`
- `from: "env"`：运行时读进程环境变量，默认视为 secret，不落盘

## 用命令读写（与桌面端变量面板同一实现）

```bash
requestbaby vars                                    # 全项目变量：各层定义 / 捕获 / 引用 / 未定义
requestbaby vars collections/orders/02-详情.json -e dev   # 某个用例每个变量的最终值、来源，以及解析后的 URL
requestbaby vars set baseUrl http://localhost:8080 -e dev
requestbaby vars set token eyJ… -e dev --secret
requestbaby vars set token eyJ… --local dev --secret    # 只在本机：envs/dev.local.json（自动加入 .gitignore）
requestbaby vars set apiKey --project --from-env MY_KEY
requestbaby vars set tenant t1 --collection collections/admin
requestbaby vars unset tenant --collection collections/admin
```

入库的环境文件放占位或 `{"from": "env"}`，各人用 `--local` 填自己的真实值。
