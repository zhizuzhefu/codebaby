# Web 界面验收

> `requestbaby guide web` · 桌面端：「新建用例」选 Web，用例编辑器的「步骤」页一行一步，响应面板「步骤」页按顺序列出每一步的截图

AI 改完前端，「接口都通过」不等于「用户能用」。Web 用例用本机的 Chrome / Edge 真的打开页面、点按钮、填表单，**每一步自动截图**，人看截图就能确认 AI 交付的界面是不是真的能用。

- 不需要 Playwright / Node / WebDriver：直接驱动本机浏览器（Chrome DevTools 协议），缺省无界面运行
- 运行和判定不调用任何大模型、不消耗 token：同样的用例每次得到同样的结论
- 每个用例一个全新的浏览器环境；同一次运行里，前面用例留下的登录态（Cookie、localStorage / sessionStorage）会带给后面的 Web 用例，登录一次即可（见 `requestbaby guide flow` 的「依赖与登录态」）；要换账号用流程步骤的 `as`

## 一个完整的用例

```json case
{
  "name": "登录后看到订单",
  "web": {
    "url": "{{webBase}}/login",
    "steps": [
      { "fill": { "label": "手机号" }, "value": "{{phone}}" },
      { "fill": { "label": "密码" }, "value": "{{password}}" },
      { "click": { "role": "button", "name": "登录" } },
      { "waitFor": { "url": "/orders" } },
      { "expect": { "role": "heading", "name": "我的订单" } },
      { "expect": { "testId": "order-status", "hasText": "已支付" } },
      { "click": { "role": "link", "name": "查看详情", "nth": 0 }, "capture": { "orderNo": { "testId": "order-no", "regex": "ORD-\\d+" } } },
      { "screenshot": "订单详情" }
    ]
  },
  "expect": { "status": 200, "body": { "url": { "contains": "/orders/" } } }
}
```

## 步骤：一步只做一件事

| 步骤 | 作用 |
|------|------|
| `{"click": 定位}` | 点击（真实的鼠标事件） |
| `{"fill": 定位, "value": "…"}` | 清空输入框再输入（真实的键盘输入，前端框架能收到） |
| `{"select": 定位, "value": "上海"}` | 下拉框选择：写选项文字或值 |
| `{"press": "Enter"}` | 按键：`Enter` / `Tab` / `Escape` / `Backspace` / `ArrowDown` / `Control+a` … |
| `{"hover": 定位}` | 鼠标悬停（展开菜单等） |
| `{"goto": "/settings"}` | 打开地址：完整地址，或相对当前页面的路径 |
| `{"back": true}` | 浏览器后退 |
| `{"waitFor": 条件}` | 一直等到条件满足，超时则失败 |
| `{"expect": 条件}` | 断言：超时内满足即通过，计入断言结果 |
| `{"screenshot": "名字"}` | 额外截一张图并命名（每一步本来就会截图） |
| `{"wait": 500}` | 静候毫秒（尽量用 `waitFor`） |

每一步都可以加：`timeoutMs`（本步等待上限，缺省 10000）、`optional: true`（找不到目标就跳过，用于可能出现的弹窗）、`capture`（本步完成后取值存为变量）。

**不用写 sleep**：每一步自动等目标出现、可见、可用、位置稳定、没被遮挡再操作；点击引起的页面跳转自动等加载完成。

## 定位：按用户看得到的东西写

| 写法 | 找的是 |
|------|--------|
| `{"role": "button", "name": "登录"}` | 角色 + 名称（和读屏软件看到的一致）。角色：button / link / textbox / checkbox / radio / combobox / heading / tab / menuitem / dialog … |
| `{"label": "手机号"}` | 表单控件的标签（`<label>`、aria-label；没有标签时用占位文字） |
| `{"text": "忘记密码"}` | 可见文字 |
| `{"placeholder": "请输入手机号"}` | 输入框占位文字 |
| `{"testId": "submit"}` | `data-testid` |
| `{"css": "#app .submit"}` | CSS 选择器：兜底用，页面改版容易失效 |
| `{"ocr": "立即预约"}` | 屏幕上看到的文字（对页面截图做文字识别，Canvas / 图片里的字用它；`guide ocr`） |

- 名称 / 文字缺省**包含即可、忽略大小写**；要完全相等加 `"exact": true`
- 匹配到多个时直接报错（避免点错），并在失败截图上框出所有匹配：加 `"nth": 0`（第 1 个）或把名称写完整
- `"hasText": "订单 123"` 在多个匹配里只要文字包含它的（如表格里某一行）
- 不知道怎么写？`requestbaby web snapshot <地址>` 列出页面上全部按钮 / 链接 / 输入框和可直接粘贴的定位

## 条件（waitFor / expect）

| 写法 | 要求 |
|------|------|
| `{"url": "/orders"}` | 当前地址包含 |
| `{"title": "我的订单"}` | 页面标题包含 |
| `{"role": "dialog"}` | 元素可见（缺省要求） |
| `{"text": "加载中", "state": "hidden"}` | 状态：visible / hidden / enabled / disabled / checked / unchecked |
| `{"role": "row", "count": 3}` | 可见的匹配元素恰好 3 个 |
| `{"label": "昵称", "value": "张三"}` | 输入框的值 |
| `{"testId": "status", "hasText": "已支付"}` | 元素文字包含 |

## 捕获

`capture` 写在任意一步上，本步完成后取值：`"url"` / `"title"`，或一个元素定位（缺省取文字，输入框取值；`attr` 取属性，`regex` 截取，有分组取第 1 组）。后面的步骤和用例用 `{{变量}}` 引用。

```json case
{
  "name": "下单后拿到订单号",
  "web": {
    "url": "{{webBase}}/cart",
    "steps": [
      { "click": { "role": "button", "name": "去结算" } },
      { "click": { "role": "button", "name": "提交订单" }, "capture": { "orderUrl": "url" } },
      { "waitFor": { "text": "下单成功" }, "capture": { "orderNo": { "text": "订单号", "regex": "订单号[:：]\\s*(\\S+)" }, "detail": { "role": "link", "name": "查看订单", "attr": "href" } } }
    ]
  }
}
```

## 断言读什么

| expect | Web 里是 |
|--------|----------|
| `status` | 打开的页面（最后一次整页加载）的 HTTP 状态 |
| `headers` | 该页面的响应头 |
| `body` | 结束时的页面：`{"url": …, "title": …}` |

## 证据：每一步都有截图

- 点击类：操作**前**截图，红框 + 圆环标出点了哪里
- 输入类：操作后截图，红框标出输入框（密码框的值不写进步骤描述，按 secret 变量规则脱敏）
- 断言 / 等待：通过时绿框标出验证到的元素
- 失败：橙框标出相关元素，另存现场——页面 HTML、无障碍树、控制台日志、网络请求
- 最后一张是结束画面；还记录浏览器版本、视口、无界面 / 有界面

证据放在 `.requestbaby/runs/<运行 ID>/<用例>/`（`send` 放在 `.requestbaby/scratch/`），运行记录里写着代码版本（提交、分支、有无未提交改动）。最近 30 次运行的证据全部保留，更早的只保留有失败的。

- `requestbaby run` 汇总里给出证据目录；失败用例列出失败截图路径（AI 可以直接打开图片看）
- `requestbaby run --report html` 生成带截图时间线的单文件报告，发给别人也能看
- `requestbaby send <用例>` 在终端列出每一步、截图文件与运行环境
- 同一提交上时过时不过的用例标为「不稳定」：结论不可信，需要人看
- 运行中就能看：终端里每完成一步打印一行；桌面端响应面板的「步骤」页跟随进度，每一步的截图一出现就能点开；`run --json-stream` 每步一条 `uiStep` 事件（带证据目录与截图文件名），AI / CI 可以边跑边读，不必等用例结束

## 设置

```json project
{
  "version": 1,
  "defaultEnv": "dev",
  "web": { "viewport": { "width": 1440, "height": 900 }, "screenshots": "steps" }
}
```

- `web.browser`：浏览器路径（缺省自动找 Chrome / Edge / Chromium；环境变量 `REQUESTBABY_BROWSER` 优先）
- `web.endpoint`：连接远程浏览器，见下一节
- `web.headed: true` / `--headed` / `REQUESTBABY_HEADED=1`：显示浏览器窗口，看着它跑
- `web.screenshots: "failure"`：只截失败、结束画面和 `screenshot` 步骤（用例很多时省空间）
- 用例里 `web.viewport` 覆盖视口，`web.timeoutMs` 覆盖每一步的缺省超时
- 之前 HTTP 用例登录拿到的 Cookie 自动带进浏览器；浏览器里登录得到的 Cookie 后面的 HTTP 用例也能用
- 页面里的 `alert` / `confirm` 自动确认，并记在那一步的说明里

## 远程浏览器

浏览器不必在本机：Docker 里的 Chrome、CI 上的浏览器服务、云浏览器、另一台机器上开了调试端口的 Chrome，都可以直接连。

```json project
{
  "version": 1,
  "web": { "endpoint": "http://chrome.internal:9222" }
}
```

- `web.endpoint`（或环境变量 `REQUESTBABY_BROWSER_URL`，优先）写 `http://主机:端口`，或云浏览器给的 `ws://…` / `wss://…`（令牌写在地址里，记录和报告里会隐去）
- 配了就不启动本机浏览器；运行结束只断开连接，不关闭远程浏览器。每个用例照样是全新的浏览器环境
- 步骤、截图、证据与本机完全相同，运行环境里「模式」记为「远程（地址）」
- 远程浏览器里的 `localhost` 是**远程那台机器**：被测页面写它能访问到的地址（本机 Docker 里用 `host.docker.internal`），`file://` 本机文件也打不开
- `requestbaby doctor` 检查远程浏览器能否连上
- 配了代理时（`requestbaby guide proxy`），调试连接也经代理；本机浏览器的页面流量同样经代理

Docker 起一个：`docker run -d -p 9222:9222 chromedp/headless-shell`，`web.endpoint` 写 `http://localhost:9222`（这时页面地址里的 localhost 指容器自己）。

## 命令

- `requestbaby new case collections/web 登录 --web "{{webBase}}/login"` 新建
- `requestbaby web snapshot "{{webBase}}/login"` 列出元素与推荐定位，并截一张图
- `requestbaby run --dry-run` 列出每个用例的步骤（不打开浏览器）
- `requestbaby doctor` 检查本机浏览器 / 远程浏览器
