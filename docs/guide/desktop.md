# Mac 桌面应用验收

> `requestbaby guide desktop` · 桌面端：「新建用例」选 Mac 桌面，步骤写法与 Web / Android 相同，响应面板「步骤」页按顺序列出每一步的截图

用 [agent-desktop](https://github.com/vercel-labs/agent-desktop) 通过 macOS 无障碍树启动 / 接管 Mac 应用，点击、输入、选择、按快捷键，**每一步自动截图**，留下「这个版本的应用在这台 Mac 上真的能用」的证据。

- 需要安装 agent-desktop（`npm install -g agent-desktop`），被测应用里不用装任何东西；运行和判定不调用任何大模型、不消耗 token
- 需要在「系统设置 → 隐私与安全性」给**运行 requestbaby 的程序**（终端 / requestbaby.app）开启「辅助功能」，截图还需要「屏幕录制」；`requestbaby doctor` 会检查（`agent-desktop permissions --guide` 带你操作）
- 默认只走无障碍语义调用，**不抢焦点、不动你的鼠标**；没有语义动作的元素会明确报错，不会悄悄改用真实鼠标
- 步骤、定位、条件、捕获、证据的写法与 Web / Android 用例一致（`requestbaby guide web`），下面只列不同之处

## 一个完整的用例

```json case
{
  "name": "文本编辑里输入并读回",
  "desktop": {
    "app": "TextEdit",
    "steps": [
      { "click": { "role": "button", "name": "New Document" }, "optional": true },
      { "fill": { "role": "textfield" }, "value": "{{greeting}}" },
      { "expect": { "role": "textfield", "value": "{{greeting}}" } },
      { "type": "，再追加一句" },
      { "press": "Meta+s" },
      { "waitFor": { "role": "button", "name": "Save" } },
      { "capture": { "body": { "role": "textfield" } }, "screenshot": "输入完成" }
    ]
  }
}
```

## 应用

| 字段 | 作用 |
|------|------|
| `app` | 应用名称（如 `TextEdit`）或 bundle ID（如 `com.apple.TextEdit`）（必填） |
| `window` | 窗口标题包含这段文字的窗口，要固定操作某个窗口时写；缺省操作唯一可见的窗口，有多个窗口（如弹出了保存对话框）时操作获得焦点的那个 |
| `restart` | 开始前先正常退出应用再重新启动；缺省**接管已在运行的应用，不动它** |

应用没运行会自动启动，并把它带到前台。退出用 `close-app` 的正常退出，**不会强杀**，应用在等你确认保存时会报错而不是丢数据。

## 步骤（与 Web / Android 不同的）

| 步骤 | 作用 |
|------|------|
| `{"click": 定位}` | 点击（无障碍的「按下」动作） |
| `{"fill": 定位, "value": "…"}` | 清空并设置输入框的值 |
| `{"select": 定位, "value": "选项"}` | 下拉框 / 菜单按钮选择选项文字 |
| `{"type": "…"}` | 往当前焦点输入框追加文字（先 click 或 fill 让它获得焦点） |
| `{"press": "Meta+s"}` | 按键 / 快捷键：`Enter` / `Escape` / `Tab` / `ArrowDown` / `Meta+s`（⌘S）/ `Control+a` …；`⌘Q` 等退出快捷键会被拒绝 |
| `{"swipe": "up"}` | 滚动窗口里最大的滚动区域，方向写手指移动的方向（与 Android 一致：up = 内容往下翻） |
| `{"launch": true}` | 重新启动应用（先正常退出再启动） |
| `{"stop": true}` | 正常退出应用 |

循环、条件、随机等待、运行本机命令：`repeat` / `if` / `wait: {"min", "max"}` / `exec`，与 Web 相同（`requestbaby guide control`）。

不支持：`hover` / `longPress` / `back` / `goto`。悬停、双击、拖拽需要真实鼠标，requestbaby 不会为了通过用例去动你的鼠标。

## 定位

写法与 Android 一致，角色用系统的名字：

| 写法 | 找的是 |
|------|--------|
| `{"role": "button", "name": "保存"}` | 角色 + 名称：button / textfield / checkbox / radiobutton / combobox / menubutton / treeitem / cell / statictext … |
| `{"text": "已保存"}` | 名称、值或描述里包含的文字 |
| `{"ocr": "已保存"}` | 屏幕上看到的文字（对窗口截图做文字识别，无障碍树里没有的内容用它；点击需要 `desktop.headed`，`guide ocr`） |
| `{"label": "名称"}` | 输入框的名称 / 描述 / 占位文字 |
| `{"id": "NewDocumentButton"}` | AXIdentifier（开发者给的稳定标识，有就优先用） |
| `{"desc": "关闭"}` | 无障碍描述 |

`role` 也认 `textbox` / `radio` / `img` / `text` 这些通用写法。`{"text": …}` 点到的静态文字会落在它所在的行 / 按钮上。匹配到多个时加 `nth`（从 0 开始）或 `exact`。不知道怎么写？先打开应用，再 `requestbaby desktop snapshot TextEdit` 列出窗口里的元素与推荐定位，并截一张图；`requestbaby desktop apps` 列出正在运行的应用名称。

不在窗口可见范围里的元素（`offscreen`）找不到：先用 `swipe` 滚动到它。

## 条件与断言

条件写法同 Web，页面条件换成 `{"title": "设置"}`（窗口标题包含）。`state` / `count` / `value` / `hasText` 同样可用。`expect.body` 是结束时的 `{"app": …, "window": …}`；Mac 桌面没有 HTTP 状态，状态列显示「完成」。`capture` 的内置来源写 `"window"`（窗口标题）/ `"app"`，或一个元素定位。

## 证据

与 Web 相同：每一步截取应用窗口（去掉阴影）并标出操作 / 验证的元素，失败时另存界面结构（`accessibility.txt`，无障碍树的文本大纲，密码框的值不写出）和应用的窗口列表；记录 macOS 版本、agent-desktop 版本、应用版本、窗口尺寸。

## 注意

- 同一时间只能操作一个应用的界面：Mac 桌面用例按顺序执行，也不要同时手动操作被测应用
- 界面特别大的应用（Electron、Xcode）读取无障碍树可能超出 8 秒预算，会提示「没读完」，此时靠后的元素可能找不到
- 登录态保存在应用里，流程的 `as`（切换用户）对它没有作用
- agent-desktop 缺省从 PATH 与常见安装位置（Homebrew / nvm / npm 全局目录）找，也可以写 `requestbaby.json` 的 `desktop.agentDesktop`，或设置环境变量 `REQUESTBABY_AGENT_DESKTOP`

## 命令

- `requestbaby new case collections/mac 保存文档 --desktop TextEdit` 新建
- `requestbaby desktop apps` 列出正在运行的应用
- `requestbaby desktop snapshot TextEdit [--window 标题]` 列出窗口里的元素与推荐定位，并截一张图
