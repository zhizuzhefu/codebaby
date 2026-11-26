# WebSocket

> `codebaby guide ws` · 桌面端：「新建用例」时选 WebSocket（或在用例编辑器左上角把协议切成 WebSocket），用例编辑器的「对话」页一行一步，响应面板「对话」页按时间列出收发的每条消息

和 HTTP 用例只差一处：请求部分写 `ws`。连接后按 `steps` 一步步收发，每一步的断言与捕获立刻生效，后面的步骤就能用。

## 一个完整的用例

```json case
{
  "name": "聊天室收发",
  "ws": {
    "url": "{{wsBase}}/chat",
    "headers": { "Authorization": "Bearer {{token}}" },
    "steps": [
      { "receive": { "type": "welcome" }, "capture": { "sid": "sid" } },
      { "send": { "type": "join", "room": 1 } },
      { "waitFor": { "type": "joined", "room": 1 } },
      { "send": { "type": "say", "text": "hi" } },
      { "waitFor": { "type": "message", "data.from": "bob" }, "capture": { "msgId": "data.id" } },
      { "send": { "type": "ack", "id": "{{msgId:number}}", "sid": "{{sid}}" } }
    ]
  },
  "expect": {
    "status": 101,
    "body": { "[*].type": { "ne": "error" } }
  }
}
```

## 步骤：一步只做一件事

| 步骤 | 作用 |
|------|------|
| `{"send": …}` | 发文本消息：字符串原样发，对象 / 数组按 JSON 发；`{{变量}}` 在发送这一刻替换（前面步骤捕获的也能用） |
| `{"sendBinary": "AQID"}` | 发二进制消息（base64） |
| `{"receive": {…}}` | 下一条消息必须满足条件；`{}` = 只要收到一条 |
| `{"waitFor": {…}}` | 一直等到满足条件的消息，中间的心跳等其他消息跳过 |
| `{"wait": 500}` | 静候 500 ms，期间收到的消息照常记录 |
| `{"close": true}` | 主动关闭（或写关闭码 `4000`）；不写也会在最后正常关闭 |

- `receive` / `waitFor` 的条件写法与 `expect.body` 完全相同：`路径 → 期望`，`$` = 整条消息（文本消息用 `{"$": "pong"}`）
- `receive` / `waitFor` 可带 `capture`：从这条消息取值存为变量
- 每步可写 `timeoutMs`（缺省用 `ws.timeoutMs`，再缺省用项目超时）；任一步失败就停止对话，报告写明第几步、期望什么、实际收到什么

## 断言读什么

| expect | WebSocket 里是 |
|--------|----------------|
| `status` | 握手状态：成功 101；被服务端拒绝时是它返回的状态码（401 / 403…） |
| `headers` | 握手响应头；服务端关闭连接时另有 `ws-close-code` / `ws-close-reason` |
| `body` | 收到的**全部消息数组**（JSON 文本已解析，其他是字符串，二进制是 base64） |

验证未登录被拒（没有步骤，只看握手）：

```json case
{
  "name": "未登录被拒",
  "ws": { "url": "{{wsBase}}/chat" },
  "expect": { "status": 401 }
}
```

没写 `expect.status` 时握手必须成功，否则用例失败（对话一步都没进行，不能算通过）。

验证服务端主动断开：

```json case
{
  "name": "说再见后服务端关闭",
  "ws": {
    "url": "{{wsBase}}/chat",
    "headers": { "Authorization": "Bearer {{token}}" },
    "steps": [{ "send": { "type": "bye" } }, { "wait": 300 }]
  },
  "expect": { "headers": { "ws-close-code": "4001" } }
}
```

## 其他

- 地址写 `ws://` / `wss://`（`http(s)://` 也接受）；`query` 写查询参数、`subprotocols` 写子协议、自签证书写 `"insecure": true`
- 集合 `collection.json` 的 `headers` 会一起带上；同一主机之前 HTTP 用例拿到的 Cookie 自动带上
- `codebaby send <用例>` 在终端按时间列出整段对话（→ 发出，← 收到）
- `codebaby run --dry-run -v` 显示等价的 websocat 命令
- `codebaby new case collections/chat 连接测试 --ws "{{wsBase}}/chat"` 新建
