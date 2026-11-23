# 发版

发布渠道只有 GitHub Release。版本号以 `Cargo.toml` 的 `[workspace.package] version` 为准，标签 `vX.Y.Z`。

## 版本号策略

- **不复用版本号**：任何改动要发给用户，都发新版本；已发布过的版本号不再覆盖附件、不再移动标签
- **补丁号 z 自动加一**：例如已发布的最高版本是 0.3.1，下一次 `make release` 就是 0.3.2。脚本自己把 `Cargo.toml` / `Cargo.lock` 改好，提交 `chore(release): vX.Y.Z` 并推送，再发布，不用手动改
- **x.y 变化需要人工确认**：`scripts/release.sh --bump minor`（0.3.z → 0.4.0）或 `--bump major`（→ 1.0.0），或者手动把 `Cargo.toml` 改成更高的 x.y。无论哪种，发布前都必须在终端里输入新版本号确认；非交互环境（脚本 / AI 代跑）要由人确认后加 `--confirm-version X.Y.Z`，值必须等于将发布的版本

## 一次发版

1. 提交并推送到 `main`（工作区干净、与 origin/main 一致）
2. `make release`（等同 `scripts/release.sh`；`--dry-run` 先看将发布哪个版本），脚本依次：
   - 检查：在 main、工作区干净、与 origin/main 一致；按上面的策略定出版本号（z 自动加一；x.y 变化要确认）
   - `cargo test` + `clippy -D warnings`
   - **本机**构建 universal DMG：Developer ID 签名 + 公证，生成 `.sha256`
   - 打包 macOS 命令行（Apple 芯片 / Intel），各带 `.sha256`；建 GitHub Release **草稿**并上传全部 6 个附件，推送 `vX.Y.Z` 标签
   - 附件数量核对无误后发布；此后用户的「检查更新」才会看到新版本

`scripts/release.sh --dry-run` 只做检查。附件数量不对时草稿保留，核对后 `gh release edit vX.Y.Z --draft=false --latest` 手动发布。

发布全程在本机完成，仓库不使用 CI（GitHub Actions）；测试与 clippy 由发布脚本在发布前跑。只发布 macOS 版（桌面端与命令行）。

版本号不复用：哪怕只想换一个安装包，也发新的补丁版本。

## 为什么签名公证在本机

Apple 证书和公证凭据只存在维护者的钥匙串里，不放进 GitHub Secrets，仓库开源后也没有可被 fork / PR 工作流窃取的密钥。代价是发版要在有证书的那台 Mac 上做。

## 用户怎么获得更新

- 命令行：`requestbaby upgrade`（`--check` 只检查，有新版本时 exit 1）。下载对应平台的包，校验 SHA256 后替换自身；在 `requestbaby.app` 里的命令行走与桌面端相同的自动更新（下载 → 校验 → 退出后原子替换），随 App 一起更新
- 桌面端：启动时与运行期间每 6 小时后台检查；发现新版本自动下载 DMG，并在暂存副本上校验（SHA256、版本、签名、与当前 App 同一团队、公证、能运行），状态栏出现「重启更新 x.y.z」；用户退出时由独立的更新程序原子替换（也可点状态栏 / 菜单栏 requestbaby →「重启并更新…」立即装，装完自动重新打开）。替换失败会换回旧版本并在下次启动时提示，日志在 `~/Library/Logs/requestbaby-update.log`，该版本不再自动重试。不能自动更新时（没有 /Applications 写权限、从 DMG 或下载目录里直接运行、开发构建）退回状态栏「新版本」手动下载 DMG。右上角「⋯」菜单有「检查更新…」「重启并更新」与自动更新开关（环境变量 `REQUESTBABY_NO_UPDATE_CHECK=1` 也可关闭）
- 首次启用：自动更新是 0.3.3 起才有的能力，更旧的版本需要手动安装一次 0.3.3 或更高版本，之后就全自动
- 检查与下载只访问 `api.github.com` 和 Release 附件，不上传任何本机信息，走与用例相同的代理规则
- 下载的附件必须有同名 `.sha256`，校验不通过不安装。这能发现下载损坏，但不能防范 Release 本身被篡改——Release 的写权限就是信任根

## 仓库迁移

发布仓库写在 `crates/requestbaby-core/src/update.rs` 的 `REPO` 常量里（环境变量 `REQUESTBABY_UPDATE_REPO=owner/name` 可临时指向别处）。迁移仓库时改这一处。检查更新用的是 GitHub 公开接口：仓库是私有时查不到版本，会显示「还没有发布版本」。
