# gRPC

> `codebaby guide grpc` · 桌面端：「新建用例」时选 gRPC（或在用例编辑器左上角把协议切成 gRPC），用例编辑器顶部是「地址 + 方法」，下面是 消息 / 元数据 / 断言 / 脚本 / 高级

和 HTTP 用例只差一处：请求部分写 `grpc`，不写 `request`。断言、捕获、变量、集合、流程、报告完全相同。

## 一个完整的用例

```json case
{
  "name": "查询订单",
  "grpc": {
    "target": "{{grpcTarget}}",
    "method": "shop.v1.OrderService/GetOrder",
    "message": { "id": "{{orderId:number}}" },
    "metadata": { "authorization": "Bearer {{token}}" }
  },
  "expect": {
    "status": "OK",
    "headers": { "x-trace-id": { "ne": "" } },
    "body": {
      "id": "{{orderId}}",
      "state": "PAID",
      "items[*].qty": { "gt": 0 }
    }
  },
  "capture": { "owner": "owner" }
}
```

| 字段 | 写什么 |
|------|--------|
| `target` | `host:port`（明文）；`grpcs://host:port` 是 TLS（缺省端口 443）。放进环境：`codebaby vars set grpcTarget localhost:50051 -e dev` |
| `method` | `包.服务/方法`，`codebaby grpc list` 列出全部 |
| `message` | 请求消息，protobuf 的 JSON 写法；字段名同 .proto（lowerCamelCase 也接受）；数字变量写 `"{{id:number}}"` |
| `messages` | 客户端流 / 双向流：按顺序发送的多条消息（与 message 二选一） |
| `metadata` | 请求元数据（相当于请求头，名称小写）；集合 `collection.json` 的 `headers` 也会一起发 |
| `timeoutMs` / `insecure` | 超时（同时作为 grpc-timeout 发给服务端）/ 自签证书跳过校验 |

## 断言读什么

| expect | gRPC 里是 |
|--------|-----------|
| `status` | gRPC 状态：`"OK"`、`"NOT_FOUND"`、`["OK", "NOT_FOUND"]`，或数字 0–16 |
| `headers` | 响应元数据（headers + trailers），服务端的错误说明在 `grpc-message` |
| `body` | 响应消息。字段名同 .proto，int64 是数字，默认值（0、""、false）也会出现，可以直接断言 |

服务端流 / 双向流的响应是**消息数组**，用 `[0]`、`[*]`、`$`、`values` 断言：

```json case
{
  "name": "订阅订单状态",
  "grpc": { "target": "{{grpcTarget}}", "method": "shop.v1.OrderService/Watch", "message": { "id": 7 } },
  "expect": {
    "status": "OK",
    "body": {
      "$": { "len": 2 },
      "[*].state": { "values": ["CREATED", "PAID"] }
    }
  }
}
```

验证错误码，同时核对说明文字：

```json case
{
  "name": "订单不存在",
  "grpc": { "target": "{{grpcTarget}}", "method": "shop.v1.OrderService/GetOrder", "message": { "id": 404 } },
  "expect": { "status": "NOT_FOUND", "headers": { "grpc-message": { "contains": "not found" } } }
}
```

状态不符时报告直接带上服务端说明：`status = OK → 实际 UNAUTHENTICATED：missing token`。

## 客户端流 / 双向流

```json case
{
  "name": "批量上传",
  "grpc": {
    "target": "{{grpcTarget}}",
    "method": "shop.v1.OrderService/Upload",
    "messages": [{ "sku": "A", "qty": 2 }, { "sku": "B", "qty": 3 }]
  },
  "expect": { "status": "OK", "body": { "items": { "len": 2 } } }
}
```

消息全部发出后结束发送，再收齐响应。需要「收到一条再决定发什么」的交互式对话，用 WebSocket 的 steps（`codebaby guide ws`）。

## 接口描述从哪来

- **服务端反射**（缺省）：服务开启了 grpc.reflection 就不需要任何配置
- **.proto 文件**：服务没开反射，或希望 `validate` / `--dry-run` 不连服务就核对方法名与消息字段，在 codebaby.json 里配置：

```json fragment
"grpc": { "protos": ["protos"], "importPaths": ["third_party"] }
```

`protos` 写文件或目录（目录下全部 .proto），相对项目根；import 搜索目录缺省 = 列出的目录 + 项目根。

## 命令

```bash
codebaby grpc list                                    # 全部方法（缺省地址 {{grpcTarget}}）
codebaby grpc describe shop.v1.OrderService/GetOrder  # 方法详情 + 请求消息骨架
codebaby new case collections/orders 查询订单 --grpc shop.v1.OrderService/GetOrder   # 自动填好消息骨架
codebaby send collections/orders/01-查询订单.json --suggest   # 真实响应 + 建议断言
codebaby run --dry-run -v                             # 每个用例的等价 grpcurl 命令
```

## 常见报错

| 报错 | 处理 |
|------|------|
| 服务端没有开启反射 | 配置 `grpc.protos` |
| 服务 X 没有方法 Y（有：…） | 方法名写错，按提示改 |
| 请求消息不符合 X：unrecognized field（可用字段：…） | 字段名写错，按提示改 |
| TLS 握手 / 证书校验失败 | 地址是 TLS 但证书自签：`"insecure": true`；或其实是明文端口：去掉 `grpcs://` |
