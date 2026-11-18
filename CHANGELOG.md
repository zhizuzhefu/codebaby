# 更新日志

每个版本的说明会原样出现在 GitHub Release 和应用内的「新版本」提示里。写法：一行一条，说用户能感知到的变化。

## 0.3.0 — 2026-11-18

- 新增 Web / Android 界面验收：第 4、5 种用例类型。每一步自动截图并标注，失败另存现场，HTML 报告内嵌步骤时间线；运行与判定不调用大模型
- Web 用本机 Chrome / Edge 驱动，也可连接 Docker / 云端的远程浏览器；Android 用 adb，远程模拟器 / 云手机写 `主机:端口` 自动连接
- 新增代理：HTTP / gRPC / WebSocket / 浏览器 / adb 全部经同一条规则，支持 socks5 / socks5h / http；不配置时跟随系统（环境变量 → macOS 系统代理）；`requestbaby proxy` 与桌面端「项目设置 → 代理」同一设置
- 新增 `requestbaby upgrade` 与桌面端「新版本」提示：检查并升级到最新版本（SHA256 校验）
- 桌面端：侧栏「新建」改为醒目的按钮；协议名统一为「HTTP / HTTPS」
- 命令行与桌面端拆分为两个独立程序，共用同一套核心
