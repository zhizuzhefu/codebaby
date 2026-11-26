# 代理

> `codebaby guide proxy` · 桌面端：项目设置 → 代理

一处设置，**所有对外连接**都走同一条规则：HTTP 用例、gRPC、WebSocket、脚本里的 `http.request`、Web 用例的浏览器（本机或远程）、Android 的网络设备与远程 adb server。

## 三种状态

| 状态 | codebaby.json | 效果 |
|------|------------------|------|
| 跟随系统 | 不写 `proxy` | 先看环境变量 `HTTPS_PROXY` / `HTTP_PROXY` / `ALL_PROXY`（`NO_PROXY` 里的直连）；没有再看系统代理设置（macOS：系统设置 → 网络 → 代理，含例外列表）；都没有就直连 |
| 使用代理 | `"enabled": true` | 全部经 `url` 指定的代理，`noProxy` 里的主机直连 |
| 不使用代理 | `"enabled": false` | 全部直连，环境变量里的代理也不用；`url` 保留，再打开不用重填 |

```json project
{
  "version": 1,
  "proxy": { "enabled": true, "url": "socks5h://{{proxyUser}}:{{proxyPass}}@10.0.0.2:1080", "noProxy": ["corp.internal", "10.0.0.0/8"] }
}
```

- `url`：`socks5://`（本机解析域名）/ `socks5h://`（域名交给代理解析：内网域名、本机 DNS 不可靠时用它）/ `http://`（HTTP CONNECT）。账号密码写 `user:pass@`，或写成 `{{变量}}` 放进 `envs/<环境>.local.json`（不入库）
- `noProxy`：`example.com`（含子域）/ `*.corp` / `10.0.0.0/8` / 单个 IP
- 本机地址（`localhost` / `127.0.0.1` / `::1`）**始终直连**：经代理访问到的会是代理那台机器
- 跟随系统时，从终端运行的命令行和从 Dock 打开的桌面端结论一致（桌面端拿不到 shell 里的环境变量，但能读到系统代理设置）。系统设置用的是 PAC 自动配置时暂不支持，按直连处理（Web 用例的浏览器仍按 PAC），`codebaby proxy` 会说明
- 代理连不上、账号密码不对时直接报错并指明是代理的问题，不会悄悄改成直连

## 命令

```bash
codebaby proxy                                   # 当前设置、实际生效的规则，并检查代理能否连上
codebaby proxy set socks5h://127.0.0.1:1080 --bypass corp.internal,10.0.0.0/8
codebaby proxy off                               # 禁用：全部直连（保留地址）
codebaby proxy on                                # 重新启用
codebaby proxy unset                             # 删除设置，跟随系统
codebaby run --no-proxy                          # 只这一次直连；--proxy <地址> 只这一次经某个代理
```

环境变量 `CODEBABY_PROXY`（代理地址，或 `off`）与 `--proxy` / `--no-proxy` 相同，优先于 codebaby.json，适合 CI。`codebaby doctor` 会显示代理规则并检查代理。

## 各类连接怎么走

- HTTP / 脚本 `http.request`：内嵌 libcurl 原生代理
- gRPC / WebSocket / 远程浏览器的调试连接：经代理建立 TCP 连接，TLS 与握手在其上进行
- 本机浏览器：浏览器连 codebaby 进程内的本地中转，中转按规则直连或经代理（浏览器本身不支持带账号密码的 SOCKS5）；WebRTC 的 UDP 被禁止绕开代理。远程浏览器里页面的流量由它所在的网络决定
- Android：adb 不支持代理，网络设备（`主机:端口`）与远程 adb server 经本地转发端口连接，adb 里看到的序列号是 `127.0.0.1:端口`（每次相同），运行环境里记录实际地址与代理
- 运行环境（Web / Android 的证据）里记录本次经哪个代理
