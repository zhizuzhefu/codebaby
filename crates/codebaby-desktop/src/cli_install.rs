//! 「安装命令行工具」：把 .app 内置的 `codebaby` CLI 链接到 /usr/local/bin。
//!
//! - App 与 CLI 同版本同构建（打包脚本一起编译、一起签名），链接后终端 / AI / CI 用的就是这一份
//! - /usr/local/bin 不可写时通过系统授权对话框（osascript）创建
//! - 用户 shell 的 PATH 里若有更靠前的其他 `codebaby`（如 ~/.cargo/bin），明确提示被遮蔽
use anyhow::{Context as _, Result, anyhow, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const LINK: &str = "/usr/local/bin/codebaby";

/// 与 App 一起分发的 CLI：可执行文件同目录下的 `codebaby`（.app 的 Contents/MacOS，开发构建的 target 目录同理）
pub fn bundled() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let cli = exe.parent()?.join("codebaby");
    cli.is_file().then_some(cli)
}

fn version_of(bin: &Path) -> Option<String> {
    let out = Command::new(bin).arg("--version").output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// 用户登录 shell 中 `codebaby` 实际解析到的路径（GUI 进程自身的 PATH 很短，不代表终端）
fn resolved_in_shell() -> Option<PathBuf> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let out = Command::new(shell).args(["-lic", "command -v codebaby"]).output().ok()?;
    let line = String::from_utf8_lossy(&out.stdout).lines().map(str::trim).rfind(|l| l.starts_with('/'))?.to_string();
    Some(PathBuf::from(line))
}

pub struct Installed {
    pub message: String,
    /// 被 PATH 中更靠前的其他 codebaby 遮蔽
    pub warning: Option<String>,
}

/// 创建 / 更新链接（阻塞：可能弹出系统授权对话框，调用方放后台线程）
pub fn install() -> Result<Installed> {
    let src = bundled().ok_or_else(|| anyhow!("App 中没有内置命令行工具（开发构建请先 cargo build -p codebaby）"))?;
    let link = Path::new(LINK);
    let already = std::fs::read_link(link).is_ok_and(|t| t == src);
    if !already {
        if link.exists() && std::fs::symlink_metadata(link).is_ok_and(|m| !m.file_type().is_symlink()) {
            bail!("{LINK} 已存在且不是链接（可能是手动复制的旧版本），请先自行删除后重试");
        }
        let direct = std::fs::create_dir_all("/usr/local/bin")
            .and_then(|_| {
                let _ = std::fs::remove_file(link);
                std::os::unix::fs::symlink(&src, link)
            })
            .is_ok();
        if !direct {
            // 需要管理员权限：系统授权对话框
            let cmd = format!("mkdir -p /usr/local/bin && ln -sfn {} {LINK}", sh_quote(&src.to_string_lossy()));
            let script = format!("do shell script \"{}\" with administrator privileges", cmd.replace('\\', "\\\\").replace('"', "\\\""));
            let out = Command::new("osascript").args(["-e", &script]).output().context("无法调用 osascript")?;
            if !out.status.success() {
                let err = String::from_utf8_lossy(&out.stderr);
                if err.contains("-128") {
                    bail!("已取消授权");
                }
                bail!("创建链接失败：{}", err.trim());
            }
        }
    }
    let version = version_of(&src).unwrap_or_else(|| "codebaby".into());
    let warning = resolved_in_shell().filter(|p| p != link && p != &src).map(|p| {
        let other = version_of(&p).unwrap_or_else(|| "未知版本".into());
        format!(
            "终端里的 codebaby 实际是 {}（{other}），排在 PATH 中 /usr/local/bin 之前，会遮蔽 App 内置的 {version}。删除它或调整 PATH 后，终端 / AI 用的才是同一版本",
            p.display()
        )
    });
    Ok(Installed {
        message: format!("{} {LINK} → App 内置的 {version}", if already { "已是最新：" } else { "已安装：" }),
        warning,
    })
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
