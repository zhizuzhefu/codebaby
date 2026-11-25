# 循环、条件、随机等待与通知

> `requestbaby guide control` · Web / Android / Mac 桌面的界面步骤通用；桌面端在步骤编辑器的动作下拉里选「重复 / 如果 / 通知」，内容用 JSON 写

把一次性的验收变成可以一直盯着的监控、轮询、联动。先看一个完整的例子：轮流点 4 个日期，出现「可预约」就截图、通知、结束；页面不对或任何一步出错也通知。

```json case
{
  "name": "号源监控",
  "regions": {
    "标题": { "x": 0.2, "y": 0.04, "w": 0.6, "h": 0.06 },
    "日期条": { "x": 0, "y": 0.13, "w": 1, "h": 0.03 },
    "状态条": { "x": 0, "y": 0.157, "w": 1, "h": 0.025 }
  },
  "android": {
    "app": "com.tencent.mm",
    "attach": true,
    "onFail": [{ "notify": "监控中断：{{error}}" }],
    "steps": [
      {
        "repeat": {
          "for": "forever",
          "cycle": { "date": ["11-28", "11-29", "11-30", "12-01"] },
          "steps": [
            { "expect": { "ocr": "选择医生", "region": "标题" } },
            { "tap": { "ocr": "{{date}}", "region": "日期条" }, "optional": true },
            { "wait": "2.5s" },
            {
              "if": {
                "when": { "ocr": "可预约", "region": "状态条" },
                "then": [
                  { "screenshot": "可预约" },
                  { "notify": "{{date}} 有号" },
                  { "break": true }
                ]
              }
            },
            { "wait": "3s-8s" }
          ]
        }
      }
    ]
  }
}
```

用到的几个写法，下面逐个说。

| 写法 | 作用 |
|------|------|
| `"wait": "3s-8s"` | 随机等待：每次在区间内随机取一个值 |
| `{"repeat": {…}}` | 重复一组步骤：按次数 / 时长 / 轮换取值，满足条件提前结束 |
| `{"if": {"when": 条件, "then": […], "else": […]}}` | 条件分支：条件只判断一次，不等待 |
| `{"break": true}` | 跳出最近的一层 `repeat`（写在 `if` 的 `then` 里：发现目标、通知完就结束） |
| `{"notify": "文字"}` | 发通知：弹本机通知、记日志、可选发 webhook |
| `"onFail": […]` | 用例失败时发的通知 |
| `"regions": {…}` | 给屏幕区域起名，步骤里写 `"region": "名字"` |

## 时长写人话

`wait`、每步的 `timeout`、`repeat` 的 `for` 都写人话：

| 写法 | 含义 |
|------|------|
| `"500ms"` `"2.5s"` `"1m"` `"8h"` `"1h30m"` | 时长 |
| `"3s-8s"` `"3-8s"` | 随机区间（含两端；前面没写单位就沿用后面的），只用于 `wait` |
| `"forever"` | 不限时，只用于 `for`（要停就点停止 / Ctrl-C） |

实际等了多久记在这一步的说明里。旧文件里的毫秒数写法仍然能读，`requestbaby fmt` 会统一改成这里的写法。

## 重复：repeat

| 字段 | 作用 |
|------|------|
| `times` | 最多重复几次 |
| `for` | 最长重复多久（`"8h"`、`"forever"`）：一轮结束时已超过就不再开始下一轮 |
| `cycle` | 轮换取值：`{"date": ["11-28", "11-29"]}`，每一轮取下一个，到头从头再来，步骤里写 `{{date}}` |
| `until` | 每轮结束后判断一次的条件（写法同 `expect`，不等待）：满足就提前结束 |
| `var` | 轮次变量名，缺省 `i`（第几轮，从 1 开始），步骤里写 `{{i}}` |
| `steps` | 每一轮执行的步骤（可以再嵌套，最多 4 层） |

- `times` / `for` / `cycle` **至少写一个**。只写 `cycle`（不写 `times` / `for`）= 每个值各来一轮就结束
- 循环里的步骤出错就整个用例失败，和平常一样；`optional: true` 的步骤可以跳过
- **证据不会越堆越多**：循环里的步骤只在失败时留截图，每开始新一轮就丢掉上一轮的步骤记录；结束时留一条汇总（共几次、为什么结束）和一张结束画面。`notify` 的记录一直保留
- **看得到还在跑**：循环里的步骤不逐条显示，改为最外层循环每开始一轮提示一次——终端打印「↻ 第 12 轮，已运行 9 分 12 秒」，桌面端响应面板顶部显示同样的文字，`--json-stream` 是 `loopProgress` 事件
- 提前结束有两种：`until`（每轮结束判断一次）和 `break`（写在 `if` 的 `then` 里，发现目标当场结束）。`break` 只能写在 `repeat` 里面

## 条件：if

```json case
{
  "name": "有号才通知",
  "android": {
    "app": "com.tencent.mm",
    "attach": true,
    "steps": [
      {
        "if": {
          "when": { "ocr": "已约满", "state": "hidden", "region": { "x": 0.5, "y": 0.23, "w": 0.5, "h": 0.07 } },
          "then": [{ "notify": "{{date}} 放号了" }],
          "else": [{ "screenshot": "仍然约满" }]
        }
      }
    ]
  },
  "cases": [{ "label": "11-28", "date": "11-28" }]
}
```

- `when` 写法同 `expect` / `waitFor`，**只判断一次**；加 `timeout` 就最多等这么久（缺省 0）
- 分支里的步骤和普通步骤一样，可以嵌套 `repeat` / `if`

## 通知：notify

监控发现目标、或者出了问题，要让人知道。`notify` 只做三件固定的事：

1. **弹本机通知**（macOS 通知中心；Linux 有 `notify-send` 就用；没有界面的机器自动跳过）
2. **追加一行到 `.requestbaby/alerts.log`**（带时间，随时 `tail -f`）
3. **发到 webhook**（配了才发）：以 `{"text": "通知内容"}` POST，飞书 / 钉钉 / 企业微信 / Slack 的机器人地址直接可用

```json fragment
"notify": "{{date}} 有号"
```

```json fragment
"notify": { "message": "{{date}} 有号", "webhook": "https://hooks.example.com/x" }
```

全项目用同一个 webhook，在 `requestbaby.json` 写一次，步骤里只写文字：

```json project
{
  "version": 1,
  "notify": { "webhook": "https://hooks.example.com/x" }
}
```

- 通知内容里 secret 变量的值自动脱敏；步骤写了 `webhook` 就用步骤的，没写用项目的，都没有就只弹通知和记日志
- webhook 发不出去（连不上、HTTP 非 2xx）算这一步失败；不想因此中断监控就加 `"optional": true`（跳过并记下原因）
- **不运行任何命令**：用例或项目里写的东西不会在你的机器上执行代码，打开别人的项目、点运行也一样安全。旧版本的 `exec` / `commands` 已经移除，`validate` 会指出来；要联动别的系统，用 webhook 接过去

### 失败通知：onFail

监控最怕悄悄停了。在 `android` / `web` / `desktop` 里写 `onFail`，**用例失败时**发这些通知（只能写 `notify`）：

```json fragment
"onFail": [{ "notify": "监控中断：{{error}}" }]
```

- `{{error}}` 是失败原因，`{{failedStep}}` 是出错的那一步；连「打不开页面 / 接管失败」也会触发
- 用 `expect` 当守卫就行：页面不对时 `expect` 失败，`onFail` 通知，不用再写嵌套的 `else`
- 通知本身失败，只记在步骤里，不改变用例的结论；取消运行、`break` 正常结束都不触发

## 给区域起名：regions

识别区域（`region`）在很多步骤里反复用时，在用例顶层起名，步骤里写名字：

```json fragment
"regions": { "日期条": { "x": 0, "y": 0.13, "w": 1, "h": 0.03 } }
```

定位里写 `"region": "日期条"`，等同于写出整块 `{x, y, w, h}`。名字没定义，`validate` 会提前说清楚。

## 长期监控

一个用例就是一个常驻监控：`repeat` 写 `"for": "forever"`（或 `"8h"`），`until` / `if` + `break` 判断到了就 `notify`，`onFail` 兜底失败。在桌面端打开项目、选中用例点运行，要停就点停止；命令行 `requestbaby run` 同理，Ctrl-C 停止。不需要任何外部脚本。

每几秒到几分钟一轮、想每轮重新接管页面时，也可以在外面套一层 shell 循环调用 `requestbaby run`，用退出码判断（退出码含义见 `requestbaby guide cli`）：

```bash
while true; do
  requestbaby run collections/monitor --name 号源监控 || echo "$(date) 有变化" >> alerts.txt
  sleep $((RANDOM % 6 + 3))
done
```

### 常驻运行占多少空间

循环里的步骤只在失败时留截图、每轮丢掉上一轮的记录，所以一次常驻运行只留汇总和结束画面，不会越跑越大；运行历史按条数自动裁剪，`requestbaby history clear --keep 20` / `--older-than 7d` 可以手动清。`alerts.log` 只追加不裁剪，每条一行，需要时自己截断。
