# 循环、条件、随机等待与回调

> `requestbaby guide control` · Web / Android / Mac 桌面的界面步骤通用；桌面端在步骤编辑器的动作下拉里选「重复 / 如果 / 运行命令」，内容用 JSON 写

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
    "onFail": [{ "exec": { "command": "./notify.sh", "env": { "MSG": "监控中断：{{error}}" } } }],
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
                  { "exec": { "command": "./notify.sh", "env": { "MSG": "{{date}} 有号" } } },
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
| `{"exec": "命令"}` | 运行一条本机命令：通知、回调、联动其他系统 |
| `"onFail": […]` | 用例失败时运行的命令（失败通知） |
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
- **证据不会越堆越多**：循环里的步骤只在失败时留截图，每开始新一轮就丢掉上一轮的步骤记录；结束时留一条汇总（共几次、为什么结束）和一张结束画面。`exec` 的记录一直保留
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
          "then": [{ "exec": { "command": "./notify.sh", "env": { "MSG": "{{date}} 放号了" } } }],
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

## 回调：exec

`exec` 用 `sh -c` 运行一条命令（Windows 用 `cmd /C`），工作目录是项目根。

```json fragment
"exec": "./notify.sh"
```

```json fragment
"exec": { "command": "curl -s -X POST https://hooks.example.com/x -d \"$MSG\"", "env": { "MSG": "{{date}} 有号" } }, "timeout": "10s"
```

- **命令本身不做变量替换**：捕获到的文字（OCR、页面内容）不能被当成命令执行。要传值写进 `env`（值支持 `{{变量}}`），命令里用 `$MSG`
- 自动带上环境变量：`REQUESTBABY_FILE`（用例文件）、`REQUESTBABY_ROOT`（项目根）、`REQUESTBABY_EVIDENCE`（本用例的证据目录，通知里要附截图时用它）
- 退出码非 0 算这一步失败；通知失败不想中断监控就加 `"optional": true`（跳过并记下原因）
- 命令的输出末尾记在这一步的说明里（secret 变量的值自动脱敏）；超时缺省 30 秒，`timeout` 调整，超时会结束命令
- 命令是你写在用例里的，requestbaby 不会替你判断它安不安全：**只运行你信任的项目**，和运行别人仓库里的 Makefile 一样

### 命名命令：长命令只写一次

通知命令往往又长又带转义，每个用例、每个分支都写一遍很难读。在项目 `requestbaby.json` 里起个名字，用例里只写名字：

```json project
{
  "version": 1,
  "commands": {
    "notify": "echo \"$(date '+%F %T') $MSG\" >> .requestbaby/alerts.log; osascript -e 'on run argv' -e 'display notification (item 1 of argv) with title \"号源提醒\"' -e 'end run' \"$MSG\""
  }
}
```

用例里：`{"exec": "notify", "env": {"MSG": "{{date}} 有号"}}`。

- `exec` 写的恰好是 `commands` 里的名字就用它；步骤里的 `env` 覆盖命名命令自带的 `env`；`timeout` 照常
- 值是命令字符串，或 `{"command": "…", "env": {…}}`；命令同样不做变量替换
- 重名时项目里的名字优先于同名的可执行文件；要运行同名程序，写路径（`./notify`）

### 失败通知：onFail

监控最怕悄悄停了。在 `android` / `web` / `desktop` 里写 `onFail`，**用例失败时**运行这些命令（只能写 `exec`）：

```json fragment
"onFail": [{ "exec": { "command": "notify", "env": { "MSG": "监控中断：{{error}}" } } }]
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

一个用例就是一个常驻监控：`repeat` 写 `"for": "forever"`（或 `"8h"`），`until` / `if` + `break` 判断到了就 `exec` 通知，`onFail` 兜底失败。在桌面端打开项目、选中用例点运行，要停就点停止；命令行 `requestbaby run` 同理，Ctrl-C 停止。不需要任何外部脚本。

每几秒到几分钟一轮、想每轮重新接管页面时，也可以在外面套一层 shell 循环调用 `requestbaby run`，用退出码判断：

```bash
while true; do
  requestbaby run collections/monitor --name 号源监控 || ./notify.sh "有变化"
  sleep $((RANDOM % 6 + 3))
done
```
