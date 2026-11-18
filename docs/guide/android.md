# Android 界面验收

> `requestbaby guide android` · 桌面端：「新建用例」选 Android，步骤写法与 Web 相同，响应面板「步骤」页按顺序列出每一步的截图

用 adb 在真机或模拟器上启动应用、点按、输入、滑动，**每一步自动截图**，留下「这个版本的应用在这台设备上真的能用」的证据。

- 只需要 adb（Android SDK Platform-Tools），设备上不用装测试框架；界面结构来自系统自带的 uiautomator
- 运行和判定不调用任何大模型、不消耗 token
- 步骤、定位、条件、捕获、证据的写法与 Web 用例一致（`requestbaby guide web`），下面只列不同之处

## 一个完整的用例

```json case
{
  "name": "登录后进入首页",
  "android": {
    "app": "com.example.shop",
    "clearData": true,
    "steps": [
      { "tap": { "text": "同意并继续" }, "optional": true },
      { "fill": { "id": "phone" }, "value": "{{phone}}" },
      { "fill": { "id": "password" }, "value": "{{password}}" },
      { "tap": { "role": "button", "name": "登录" } },
      { "waitFor": { "activity": "MainActivity" } },
      { "expect": { "text": "我的订单" } },
      { "swipe": "up" },
      { "tap": { "id": "order_item", "nth": 0 }, "capture": { "orderNo": { "id": "order_no" } } },
      { "back": true },
      { "screenshot": "订单列表" }
    ]
  }
}
```

## 应用

| 字段 | 作用 |
|------|------|
| `app` | 包名（必填） |
| `activity` | 启动的 Activity（缺省用应用的启动入口） |
| `apk` | 先安装这个 APK（相对项目根）；一次运行内只装一次 |
| `clearData` | 启动前清除应用数据，相当于全新安装 |

## 步骤（与 Web 不同的）

| 步骤 | 作用 |
|------|------|
| `{"tap": 定位}` | 点按（写 `click` 也行） |
| `{"longPress": 定位}` | 长按 |
| `{"fill": 定位, "value": "…"}` | 点进输入框、清空、输入 |
| `{"type": "…"}` | 往当前焦点输入文字 |
| `{"press": "Back"}` | 按键：`Back` / `Home` / `Enter` / `Backspace` / `KEYCODE_*` |
| `{"swipe": "up"}` | 滑动屏幕，写手指移动的方向：up / down / left / right |
| `{"launch": true, "clearData": true}` | （重新）启动应用 |
| `{"stop": true}` | 关闭应用 |

输入中文需要设备装 [ADBKeyBoard](https://github.com/senzhk/ADBKeyBoard) 输入法（装好即可，requestbaby 输入时临时切换，用完切回）。

## 定位

| 写法 | 找的是 |
|------|--------|
| `{"id": "phone"}` | resource-id：写全 `com.example.shop:id/phone` 或只写 `phone` |
| `{"text": "登录"}` | 文字 |
| `{"desc": "返回"}` | 无障碍描述（content-desc，图标按钮常用） |
| `{"label": "手机号"}` | 输入框的提示文字 |
| `{"role": "button", "name": "登录"}` | 控件类型 + 文字：button / textbox / checkbox / switch / image … |

同一个 id 的列表项可以写 `{"id": "title", "text": "订单 123"}` 组合区分，或用 `nth`。不知道怎么写？`requestbaby android snapshot` 列出当前界面的元素与推荐定位。

## 条件与断言

条件写法同 Web，页面条件换成 `{"activity": "MainActivity"}`（当前界面包含）。`expect.body` 是结束时的 `{"package": …, "activity": …}`；Android 没有 HTTP 状态，状态列显示「完成」。

## 证据

与 Web 相同：每一步截图并标出操作 / 验证的元素，失败时另存界面结构（uiautomator XML）、本用例期间的 logcat 和崩溃日志；记录设备型号、系统版本、屏幕尺寸、应用版本。截图等比缩小到最长边 1600 像素。

## 设备

- 只连了一台设备时自动使用；多台时用环境变量 `ANDROID_SERIAL` 或 `requestbaby.json` 的 `android.serial` 指定
- adb 缺省从 PATH / `ANDROID_HOME` 找，或写 `android.adb`
- Android 用例按顺序执行（一台设备同一时间只跑一个）
- `requestbaby android devices` 列出设备；`requestbaby doctor` 检查 adb 与设备

## 远程设备

模拟器 / 手机不必接在本机：远程模拟器、云手机、Docker 里的 Android、无线调试的真机都可以用。

```json project
{
  "version": 1,
  "android": { "serial": "10.0.0.8:5555" }
}
```

- **序列号写 `主机:端口`**：没连上时自动 `adb connect`（之前的连接掉线停在 offline 时先断开再连），连不上会说明原因
- **设备接在另一台机器上**：那台机器运行 `adb kill-server && adb -a server nodaemon`，这边写 `"android": { "server": "那台机器:5037" }`，相当于每条 adb 命令加 `-H -P`；再按需用 `serial` 选设备
- 步骤、截图、证据与本机完全相同，运行环境里记录连接方式；网络慢时截图和读取界面会慢一些，`timeoutMs` 可适当调大
- `ANDROID_SERIAL=10.0.0.8:5555 requestbaby android devices` 连接并列出设备
- 配了代理时（`requestbaby guide proxy`），网络设备和远程 adb server 经代理连接（adb 本身不支持代理，requestbaby 开本地转发端口）

## 命令

- `requestbaby new case collections/app 登录 --android com.example.shop` 新建
- `requestbaby android snapshot` 列出当前界面的元素与推荐定位，并截一张图
