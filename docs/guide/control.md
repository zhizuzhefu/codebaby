# 循环、条件、随机等待与回调

> `requestbaby guide control` · Web / Android / Mac 桌面的界面步骤通用；桌面端在步骤编辑器的动作下拉里选「重复 / 如果 / 运行命令」，内容用 JSON 写

界面步骤里有四种「控制」写法，把一次性的验收变成可以反复跑的监控、轮询、联动：

| 写法 | 作用 |
|------|------|
| `{"wait": {"min": 3000, "max": 8000}}` | 随机等待：每次执行在区间内随机取一个值（避免固定节奏） |
| `{"repeat": {…}}` | 重复执行一组步骤：按次数 / 总时长，满足条件提前结束 |
| `{"if": {"when": 条件, "then": […], "else": […]}}` | 条件分支：条件只判断一次，不等待 |
| `{"break": true}` | 跳出最近的一层 `repeat`（写在 `if` 的 `then` 里：发现目标、通知完就结束监控） |
| `{"exec": "命令"}` | 运行一条本机 shell 命令：通知、回调、联动其他系统 |

## 随机等待

`wait` 写毫秒数不变，写 `{"min", "max"}` 就是随机区间（含两端）。实际等了多久记在这一步的说明里。

## 重复：repeat

```json case
{
  "name": "刷新直到有号",
  "android": {
    "app": "com.tencent.mm",
    "attach": true,
    "steps": [
      {
        "repeat": {
          "forMs": 3600000,
          "until": { "ocr": "立即预约", "region": { "x": 0, "y": 0.2, "w": 1, "h": 0.5 } },
          "steps": [
            { "tap": { "ocr": "11-28", "region": { "x": 0, "y": 0.13, "w": 1, "h": 0.03 } } },
            { "wait": { "min": 3000, "max": 8000 } },
            { "tap": { "ocr": "11-29", "region": { "x": 0, "y": 0.13, "w": 1, "h": 0.03 } } },
            { "wait": { "min": 3000, "max": 8000 } }
          ]
        }
      },
      { "exec": "./notify.sh 有号了" }
    ]
  }
}
```

| 字段 | 作用 |
|------|------|
| `times` | 最多重复几次 |
| `forMs` | 最长重复多久（毫秒）：一轮结束时已超过就不再开始下一轮 |
| `until` | 每轮结束后判断一次的条件（写法同 `expect`，不等待）：满足就提前结束 |
| `var` | 轮次变量名，缺省 `i`（第几轮，从 1 开始），步骤里写 `{{i}}` |
| `steps` | 每一轮执行的步骤（可以再嵌套，最多 4 层） |

- `times` 和 `forMs` **至少写一个**，避免无限循环。想一直跑：`forMs` 写很大的值，或在外面用 shell 循环调用 `requestbaby run`（见下面「长期监控」）
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
          "then": [{ "exec": { "command": "./notify.sh \"$MSG\"", "env": { "MSG": "{{date}} 放号了" } } }],
          "else": [{ "screenshot": "仍然约满" }]
        }
      }
    ]
  },
  "cases": [{ "label": "11-28", "date": "11-28" }]
}
```

- `when` 写法同 `expect` / `waitFor`，**只判断一次**；加 `timeoutMs` 就最多等这么久（缺省 0）
- 分支里的步骤和普通步骤一样，可以嵌套 `repeat` / `if`

## 回调：exec

`exec` 用 `sh -c` 运行一条命令（Windows 用 `cmd /C`），工作目录是项目根。

```json fragment
"exec": "./notify.sh"
```

```json fragment
"exec": { "command": "curl -s -X POST https://hooks.example.com/x -d \"$MSG\"", "env": { "MSG": "{{date}} 有号" } }, "timeoutMs": 10000
```

- **命令本身不做变量替换**：捕获到的文字（OCR、页面内容）不能被当成命令执行。要传值写进 `env`（值支持 `{{变量}}`），命令里用 `$MSG`
- 自动带上环境变量：`REQUESTBABY_FILE`（用例文件）、`REQUESTBABY_ROOT`（项目根）、`REQUESTBABY_EVIDENCE`（本用例的证据目录，通知里要附截图时用它）
- 退出码非 0 算这一步失败；通知失败不想中断监控就加 `"optional": true`（跳过并记下原因）
- 命令的输出末尾记在这一步的说明里（secret 变量的值自动脱敏）；超时缺省 30 秒，`timeoutMs` 调整，超时会结束命令
- 命令是你写在用例里的，requestbaby 不会替你判断它安不安全：**只运行你信任的项目**，和运行别人仓库里的 Makefile 一样

## 长期监控

一次 `requestbaby run` 跑完就结束。要一直盯着，有两种办法：

1. 用例里 `repeat` + `forMs`（或 `times`），用 `until` / `if` 判断到了就 `exec` 通知——一次运行完成整个监控
2. 外面套一层 shell 循环，每轮跑一遍用例，用退出码判断：

```bash
while true; do
  requestbaby run collections/monitor --name 号源监控 || ./notify.sh "有变化"
  sleep $((RANDOM % 6 + 3))
done
```

要一直盯着，也可以把 `forMs` 写得足够大（如一年 `31536000000`），一个用例就是一个常驻监控：在桌面端打开项目、选中用例点运行，要停就点停止；不需要任何外部脚本。办法 2 每轮重新接管页面、证据按运行保留，适合几秒到几分钟一轮；办法 1 一次运行内连续轮询，不用反复启动，适合高频。
