# 配方：鉴权登录

> `requestbaby guide auth`

目标：登录一次，后面所有用例自动带上 token。

1. 账号放进环境（密码只在本机）：

```bash
requestbaby vars set username tester -e dev
requestbaby vars set password 'p@ss' --local dev --secret
```

2. `collections/01-auth/01-登录.json` 捕获 token：

```json case
{
  "name": "登录",
  "request": {
    "method": "POST",
    "url": "{{baseUrl}}/auth/login",
    "body": { "json": { "username": "{{username}}", "password": "{{password}}" } }
  },
  "expect": { "status": 200, "body": { "code": 0, "data.token": { "type": "string" } } },
  "capture": { "token": "data.token" }
}
```

3. 需要鉴权的集合在 `collection.json` 写一次请求头，目录下所有用例继承：

```json collection
{
  "name": "订单",
  "headers": { "Authorization": "Bearer {{token}}" }
}
```

```json case
{
  "name": "订单列表",
  "request": { "method": "GET", "url": "{{baseUrl}}/orders" },
  "expect": { "status": 200, "body": { "code": 0 } }
}
```

## 变体

- **Cookie 会话**：默认自动保存响应的 Set-Cookie，后续同域请求自动携带，不用捕获
- **固定 token**（不走登录）：`requestbaby vars set token eyJ… --local dev --secret`，同样在 collection.json 引用 `{{token}}`
- **验证未登录被拒**：单个用例里把请求头覆盖掉

```json case
{
  "name": "未登录被拒",
  "request": {
    "method": "GET",
    "url": "{{baseUrl}}/orders",
    "headers": { "Authorization": { "value": "", "disabled": true } }
  },
  "expect": { "status": 401 }
}
```
