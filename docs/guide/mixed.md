# 配方：多端混合（API + Web + 手机）

> `codebaby guide mixed`

目标：买家用接口下单 → 运营在网页后台审核通过 → 在手机 App 里确认订单已通过 → 再用接口复核状态。四个端的用例放进**同一个流程**，一次跑完，得到一个结论、一份报告。

核心只有一条：**前一个用例 `capture` 的值，后面任何端的用例都能用 `{{变量}}` 引用。**

```text
collections/
├── 05-web-后台登录/01-登录.json       运营登录后台（登录态沿用到整个流程）
├── 10-api-下单/01-下单.json           capture: orderNo
├── 20-web-后台审核/01-审核通过.json    fill {{orderNo}}，点「通过」
├── 30-android-确认/01-订单已通过.json  在手机里找到 {{orderNo}}，看到「已通过」
└── 40-api-复核/01-状态.json           expect 状态是 approved
```

## 1. 接口：下单，取出订单号

```json case
{
  "name": "买家下单",
  "request": {
    "method": "POST",
    "url": "{{baseUrl}}/orders",
    "headers": { "Authorization": "Bearer {{buyerToken}}" },
    "body": { "json": { "sku": "{{sku}}", "qty": 1 } }
  },
  "expect": { "status": 200, "body": { "code": 0, "data.orderNo": { "type": "string" } } },
  "capture": { "orderNo": "data.orderNo" }
}
```

## 2. 网页：后台搜索这张订单并审核通过

```json case
{
  "name": "运营审核通过",
  "web": {
    "url": "{{adminBase}}/orders",
    "timeout": "15s",
    "steps": [
      { "fill": { "label": "订单号" }, "value": "{{orderNo}}" },
      { "click": { "role": "button", "name": "搜索" } },
      { "waitFor": { "testId": "order-row", "hasText": "{{orderNo}}" } },
      { "click": { "role": "button", "name": "通过" } },
      { "expect": { "testId": "order-status", "hasText": "已通过" } }
    ]
  }
}
```

## 3. 手机：在 App 里确认这张订单

```json case
{
  "name": "手机里订单已通过",
  "android": {
    "app": "com.example.shop",
    "timeout": "15s",
    "steps": [
      { "tap": { "text": "同意并继续" }, "optional": true },
      { "tap": { "text": "我的订单" } },
      { "waitFor": { "text": "{{orderNo}}" }, "timeout": "30s" },
      { "expect": { "text": "已通过" } },
      { "screenshot": "订单已通过" }
    ]
  }
}
```

## 4. 接口：复核状态

```json case
{
  "name": "订单状态复核",
  "request": {
    "method": "GET",
    "url": "{{baseUrl}}/orders/{{orderNo}}",
    "headers": { "Authorization": "Bearer {{buyerToken}}" }
  },
  "expect": { "status": 200, "body": { "data.status": "approved" } }
}
```

## 5. 串成流程

```json project
{
  "version": 1,
  "defaultEnv": "dev",
  "workflows": {
    "admin-login": { "steps": [{ "run": "collections/05-web-后台登录" }] },
    "order-review": {
      "needs": ["admin-login"],
      "steps": [
        { "name": "接口下单", "run": "collections/10-api-下单" },
        { "name": "后台审核", "run": "collections/20-web-后台审核" },
        { "name": "手机确认", "run": "collections/30-android-确认" },
        { "name": "接口复核", "run": "collections/40-api-复核" }
      ]
    }
  }
}
```

```bash
codebaby workflow order-review
codebaby export -f text      # 结论 + 按真实先后排列的时间线 + 失败详情，可直接贴给人或 AI
```

## 几个要点

- **数据怎么传**：API 用 `capture` 从响应里取；Web / Android / Mac 桌面在任意一步上写 `capture`，从页面 / 界面的文字、输入框的值、属性、URL 里取（可加 `regex` 截取）。后面的用例在 URL、请求体、`fill` 的 `value`、`waitFor` 的文字里直接写 `{{orderNo}}`
- **登录态**：Web 的登录态（Cookie、网页存储）沿用到整个流程，登录做成被 `needs` 依赖的流程，不重复登录（`guide flow`）；Android / Mac 桌面的登录态在手机 / 应用里；API 的 token 用 `capture` 取出来，放进后面的请求头（`guide auth`）
- **顺序和隔离**：界面用例与有依赖的用例一律串行，所以 `capture` 一定先于引用；每个 Web 用例各自开一个全新的浏览器，一台手机同一时间只跑一个用例
- **异步的延迟**：手机里过几秒才出现的数据，用 `waitFor` 加 `timeout`（如 `"30s"`），不要写 `wait`
- **失败了看哪里**：`export` 的 HTML 里每个用例标着自己的端（API / Web / Android / Mac 桌面），顶部的时间线把所有端的每一步排在一起，点一行跳到对应步骤；失败现场（页面结构、控制台、网络请求、logcat）嵌在报告里
- **要一直盯着**：把其中某个界面用例写成监控（`guide control`），失败时用 `onFail` 通知

## 怎么写才不容易错（给 AI 的顺序）

1. 先分别摸清每一端有什么：`codebaby web snapshot <网页地址>`、`codebaby android snapshot`（小程序等读不到结构的加 `--ocr`），拿到可以直接粘贴的定位；接口先看响应：`codebaby send <用例> --suggest`
2. 每个用例**单独**写好、单独 `validate`、单独跑通。依赖前面 `capture` 的变量，调试时先用 `codebaby vars set` 临时填上（`guide vars`）
3. 再串成流程，`codebaby verify` 看覆盖是否完整
4. 每一端的写法、定位、证据细节见 `guide web` / `guide android` / `guide desktop` / `guide ocr`
