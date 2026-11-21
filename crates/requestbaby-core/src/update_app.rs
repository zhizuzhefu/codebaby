//! macOS 桌面端自动更新：后台下载 DMG → 在暂存副本上完整校验 → 退出后由独立的更新程序原子替换。
//!
//! 设计目标是「失败了也不会把用户的 App 弄坏」：
//! 1. 替换之前的所有工作都在旁边的暂存副本上做（下载、SHA256、挂载 DMG、拷出 .app、签名 / 团队 / 公证 / 版本 / 能运行），
//!    任何一步失败 = 已安装的 App 一个字节都没动，下次再试
//! 2. 替换由一个**拷到 .app 之外**的更新程序完成：等所有 requestbaby 进程退出（不在运行中的 App 下面换文件），
//!    用一次 `renamex_np(RENAME_SWAP)` 原子交换新旧两个 .app（任何时刻都有一个完整可用的 App）
//! 3. 交换后立刻复检新 App，不对就原地换回旧的，并把原因写进结果文件、下次启动时告诉用户
//! 4. 全程写日志（~/Library/Logs/requestbaby-update.log）；更新程序已在运行 / 没有写权限 / 从 DMG 里直接运行 / 被系统隔离运行时，
//!    都在动手之前就放弃，退回「下载 DMG 手动拖拽」
use crate::engine::proxy::ProxyPolicy;
use crate::update::{self, Release};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// 本进程是否已经交出了更新任务（「立即重启更新」与退出时自动安装不会重复启动更新程序）
static SPAWNED: AtomicBool = AtomicBool::new(false);

/// 应用级数据目录（与桌面端的最近项目、偏好同目录）
pub fn app_support_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    Some(if cfg!(target_os = "macos") { home.join("Library/Application Support/requestbaby") } else { home.join(".config/requestbaby") })
}

fn updates_dir() -> Option<PathBuf> {
    Some(app_support_dir()?.join("updates"))
}

fn log_file() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    Some(if cfg!(target_os = "macos") { home.join("Library/Logs/requestbaby-update.log") } else { updates_dir()?.join("update.log") })
}

fn log(msg: &str) {
    let line = format!("[{}] {msg}\n", crate::history::now_ms());
    eprint!("{line}");
    if let Some(f) = log_file() {
        if let Some(d) = f.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        use std::io::Write;
        if let Ok(mut h) = std::fs::OpenOptions::new().create(true).append(true).open(f) {
            let _ = h.write_all(line.as_bytes());
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 状态文件：已暂存待安装的更新 / 上次更新的结果
// ---------------------------------------------------------------------------------------------

/// 已下载、校验完、暂存在旁边的更新
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub version: String,
    /// 被替换的 App（.app 目录）
    pub app: PathBuf,
    /// 暂存的新 App（与被替换的 App 在同一目录，保证同卷原子交换）
    pub staged: PathBuf,
    pub staged_at_ms: u64,
}

/// 上一次替换的结果（更新程序写，下次启动的 App 读一次并删除）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    pub version: String,
    pub ok: bool,
    pub message: String,
    pub at_ms: u64,
}

fn pending_file() -> Option<PathBuf> {
    Some(updates_dir()?.join("pending.json"))
}

fn outcome_file() -> Option<PathBuf> {
    Some(updates_dir()?.join("outcome.json"))
}

fn write_json<T: Serialize>(path: Option<PathBuf>, v: &T) {
    let Some(p) = path else { return };
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(t) = serde_json::to_string_pretty(v) {
        let _ = crate::fsx::write(p, t);
    }
}

/// 待安装的更新：暂存副本还在、版本确实比当前新、被替换的就是当前这个 App 才算数
pub fn read_pending() -> Option<Pending> {
    let p: Pending = serde_json::from_str(&std::fs::read_to_string(pending_file()?).ok()?).ok()?;
    let valid = p.staged.join("Contents/Info.plist").is_file() && update::is_newer(&p.version, update::CURRENT) && current_app().as_deref() == Some(p.app.as_path());
    valid.then_some(p)
}

pub fn clear_pending() {
    if let Some(f) = pending_file() {
        let _ = std::fs::remove_file(f);
    }
}

/// 读出上次更新的结果并删除（只提示一次）
pub fn take_outcome() -> Option<Outcome> {
    let f = outcome_file()?;
    let o: Outcome = serde_json::from_str(&std::fs::read_to_string(&f).ok()?).ok()?;
    let _ = std::fs::remove_file(f);
    Some(o)
}

fn write_outcome(version: &str, ok: bool, message: String) {
    write_json(outcome_file(), &Outcome { version: version.into(), ok, message, at_ms: crate::history::now_ms() });
}

// ---------------------------------------------------------------------------------------------
// 当前 App 能不能自更新
// ---------------------------------------------------------------------------------------------

/// 当前进程所在的 .app（从 .app 里启动才有；开发构建没有）
pub fn current_app() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let app = exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app"))?;
    app.join("Contents/MacOS").is_dir().then(|| app.to_path_buf())
}

/// 不能自动更新的原因（None = 可以）。判断在动手之前做，不满足就退回手动下载 DMG
pub fn self_update_blocker(app: &Path) -> Option<String> {
    if !cfg!(target_os = "macos") {
        return Some("自动更新只支持 macOS".into());
    }
    let s = app.to_string_lossy();
    if s.contains("/AppTranslocation/") || s.starts_with("/Volumes/") {
        return Some("App 正从下载的压缩包 / DMG 里直接运行：先把它拖进「应用程序」再打开".into());
    }
    let Some(parent) = app.parent() else { return Some("找不到 App 所在目录".into()) };
    let probe = parent.join(format!(".requestbaby-write-test-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            None
        }
        Err(_) => Some(format!("没有权限修改 {}：用管理员账号安装，或手动下载 DMG 替换", parent.display())),
    }
}

fn staging_path(app: &Path, version: &str) -> PathBuf {
    app.with_file_name(format!(".requestbaby-update-{version}.app"))
}

// ---------------------------------------------------------------------------------------------
// 校验一个 .app
// ---------------------------------------------------------------------------------------------

fn run(cmd: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(cmd).args(args).stdin(Stdio::null()).output().with_context(|| format!("运行 {cmd} 失败"))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if !out.status.success() {
        bail!("{cmd} {} 失败：{}", args.join(" "), text.trim().lines().last().unwrap_or_default());
    }
    Ok(text)
}

fn plist_value(app: &Path, key: &str) -> Result<String> {
    let plist = app.join("Contents/Info.plist");
    Ok(run("/usr/bin/plutil", &["-extract", key, "raw", "-o", "-", &plist.to_string_lossy()])?.trim().to_string())
}

/// 签名里的团队标识（codesign -dv 输出在 stderr）
fn team_of(app: &Path) -> Result<String> {
    let text = run("/usr/bin/codesign", &["-dv", "--verbose=2", &app.to_string_lossy()])?;
    text.lines()
        .find_map(|l| l.strip_prefix("TeamIdentifier="))
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty() && t != "not set")
        .ok_or_else(|| anyhow!("{} 没有 Developer ID 团队签名", app.display()))
}

/// 新 App 必须：版本对、同一个 bundle ID、签名完整、与当前 App 同一个团队、通过系统的公证评估、自带命令行能运行
fn verify_bundle(candidate: &Path, version: &str, installed: &Path) -> Result<()> {
    let got = plist_value(candidate, "CFBundleShortVersionString")?;
    if got != version {
        bail!("安装包里的版本是 {got}，不是 {version}");
    }
    let (id_new, id_old) = (plist_value(candidate, "CFBundleIdentifier")?, plist_value(installed, "CFBundleIdentifier")?);
    if id_new != id_old {
        bail!("安装包的 bundle ID 是 {id_new}，与当前的 {id_old} 不同");
    }
    let path = candidate.to_string_lossy();
    run("/usr/bin/codesign", &["--verify", "--deep", "--strict", &path]).context("签名校验不通过")?;
    let (team_new, team_old) = (team_of(candidate)?, team_of(installed)?);
    if team_new != team_old {
        bail!("安装包的签名团队是 {team_new}，与当前的 {team_old} 不同，拒绝安装");
    }
    run("/usr/sbin/spctl", &["--assess", "--type", "execute", &path]).context("系统不认可这个 App（没有通过公证 / Gatekeeper 评估）")?;
    let cli = candidate.join("Contents/MacOS/requestbaby");
    if cli.is_file() {
        let shown = run(&cli.to_string_lossy(), &["--version"]).context("新版本无法运行")?;
        if !shown.contains(version) {
            bail!("新版本自检报告的版本是「{}」，不是 {version}", shown.trim());
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// 暂存：下载 → 挂载 → 拷出 → 校验
// ---------------------------------------------------------------------------------------------

/// 挂载的 DMG：离开作用域一定卸载
struct Mount(PathBuf);

impl Drop for Mount {
    fn drop(&mut self) {
        let p = self.0.to_string_lossy().to_string();
        if run("/usr/bin/hdiutil", &["detach", &p, "-quiet"]).is_err() {
            let _ = run("/usr/bin/hdiutil", &["detach", &p, "-force", "-quiet"]);
        }
    }
}

/// 下载并校验 DMG，在被替换的 App 旁边准备好完整校验过的新 App；返回待安装信息（同时写进状态文件）。
/// 失败时已安装的 App 没有任何改动，临时文件也已清理
pub fn stage(rel: &Release, proxy: &ProxyPolicy) -> Result<Pending> {
    let app = current_app().ok_or_else(|| anyhow!("当前不是从 .app 里运行的（开发构建），不能自动更新"))?;
    if let Some(why) = self_update_blocker(&app) {
        bail!("{why}");
    }
    if let Some(p) = read_pending().filter(|p| p.version == rel.version) {
        return Ok(p);
    }
    let asset = rel.dmg_asset().ok_or_else(|| anyhow!("这个版本没有 DMG 安装包"))?;
    let work = updates_dir().ok_or_else(|| anyhow!("找不到应用数据目录"))?.join(&rel.version);
    let _ = std::fs::remove_dir_all(&work);
    let staged = staging_path(&app, &rel.version);
    let partial = app.with_file_name(format!(".requestbaby-update-{}.partial", rel.version));
    let result = (|| -> Result<Pending> {
        log(&format!("下载 {}（{:.1} MB）", asset.name, asset.size as f64 / 1_048_576.0));
        let dmg = update::download(rel, asset, &work, proxy)?;
        let mnt = work.join("mnt");
        std::fs::create_dir_all(&mnt)?;
        let _ = std::fs::remove_dir_all(&staged);
        let _ = std::fs::remove_dir_all(&partial);
        {
            run("/usr/bin/hdiutil", &["attach", "-nobrowse", "-readonly", "-noautoopen", "-noverify", "-quiet", "-mountpoint", &mnt.to_string_lossy(), &dmg.to_string_lossy()]).context("挂载安装包失败")?;
            let _mount = Mount(mnt.clone());
            let name = app.file_name().map(|n| n.to_os_string()).unwrap_or_default();
            let src = if mnt.join(&name).is_dir() {
                mnt.join(&name)
            } else {
                std::fs::read_dir(&mnt)?.flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e == "app")).ok_or_else(|| anyhow!("安装包里没有 .app"))?
            };
            // ditto 保留签名、扩展属性与权限
            run("/usr/bin/ditto", &[&src.to_string_lossy(), &partial.to_string_lossy()]).context("拷出新版本失败（磁盘空间不足？）")?;
        }
        verify_bundle(&partial, &rel.version, &app)?;
        std::fs::rename(&partial, &staged).context("暂存新版本失败")?;
        let p = Pending { version: rel.version.clone(), app: app.clone(), staged: staged.clone(), staged_at_ms: crate::history::now_ms() };
        write_json(pending_file(), &p);
        log(&format!("新版本 {} 已下载并校验，暂存在 {}", rel.version, staged.display()));
        Ok(p)
    })();
    let _ = std::fs::remove_dir_all(&work);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&partial);
        let _ = std::fs::remove_dir_all(&staged);
        log(&format!("暂存 {} 失败：{:#}", rel.version, result.as_ref().err().unwrap()));
    }
    result
}

/// 清理旧的暂存：已安装 / 过期版本的暂存副本与下载残留
pub fn cleanup_stale() {
    let keep = read_pending();
    if let Some(app) = current_app() {
        if let Some(parent) = app.parent() {
            for e in std::fs::read_dir(parent).into_iter().flatten().flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                let stale = (n.starts_with(".requestbaby-update-") || n.starts_with(".requestbaby-old-")) && keep.as_ref().is_none_or(|k| k.staged != e.path());
                if stale {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
    }
    if let Some(dir) = updates_dir() {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            // 下载目录（以版本号命名）与用完的更新程序副本
            if e.path().is_dir() || n.starts_with("updater-") {
                let _ = std::fs::remove_dir_all(e.path()).or_else(|_| std::fs::remove_file(e.path()));
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 替换
// ---------------------------------------------------------------------------------------------

/// 交给独立的更新程序：它拷到 .app 之外运行，等所有 requestbaby 进程退出后替换。
/// relaunch = 替换完成后重新打开 App。每个进程只会交出一次
pub fn spawn_apply(p: &Pending, relaunch: bool, wait_secs: u64) -> Result<()> {
    if SPAWNED.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    let r = (|| -> Result<()> {
        let cli = p.app.join("Contents/MacOS/requestbaby");
        if !cli.is_file() {
            bail!("App 里没有命令行程序，无法执行更新");
        }
        let dir = updates_dir().ok_or_else(|| anyhow!("找不到应用数据目录"))?;
        std::fs::create_dir_all(&dir)?;
        let helper = dir.join(format!("updater-{}", std::process::id()));
        std::fs::copy(&cli, &helper).context("准备更新程序失败")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755))?;
        }
        let mut cmd = Command::new(&helper);
        cmd.args(["__apply-update", "--pid", &std::process::id().to_string(), "--app"])
            .arg(&p.app)
            .arg("--staged")
            .arg(&p.staged)
            .args(["--version", &p.version, "--wait-secs", &wait_secs.to_string()]);
        if relaunch {
            cmd.arg("--relaunch");
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().context("启动更新程序失败")?;
        log(&format!("已交给更新程序：{} → {}（重启 = {relaunch}）", update::CURRENT, p.version));
        Ok(())
    })();
    if r.is_err() {
        SPAWNED.store(false, Ordering::SeqCst);
    }
    r
}

pub struct ApplyArgs {
    pub pid: u32,
    pub app: PathBuf,
    pub staged: PathBuf,
    pub version: String,
    pub relaunch: bool,
    pub wait_secs: u64,
}

/// 属于这个 .app 的运行中进程（可执行文件路径在 app 目录下）
fn running_under(app: &Path) -> Vec<u32> {
    let Ok(out) = Command::new("/bin/ps").args(["-axo", "pid=,comm="]).output() else { return vec![] };
    let prefix = format!("{}/", app.to_string_lossy());
    let me = std::process::id();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let l = l.trim_start();
            let (pid, comm) = l.split_once(' ')?;
            let pid: u32 = pid.parse().ok()?;
            (pid != me && comm.trim().starts_with(&prefix)).then_some(pid)
        })
        .collect()
}

fn pid_alive(pid: u32) -> bool {
    // SAFETY: kill(pid, 0) 只探测进程是否存在，不发送信号
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) }
}

#[cfg(not(target_os = "macos"))]
fn swap(_: &Path, _: &Path) -> std::io::Result<()> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}

#[cfg(target_os = "macos")]
fn swap(a: &Path, b: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let (ca, cb) = (std::ffi::CString::new(a.as_os_str().as_bytes())?, std::ffi::CString::new(b.as_os_str().as_bytes())?);
    // SAFETY: 两个以 NUL 结尾的有效路径
    let rc = unsafe { libc::renamex_np(ca.as_ptr(), cb.as_ptr(), libc::RENAME_SWAP) };
    if rc == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
}

/// 更新程序的主体：等退出 → 再校验 → 原子交换 → 复检（不对就换回）→ 清理 / 重启。
/// 成败都写结果文件；失败时已安装的 App 保持原样
pub fn apply(a: &ApplyArgs) -> Result<()> {
    let lock = updates_dir().map(|d| d.join("apply.lock"));
    if let Some(l) = &lock {
        // 上一个更新程序崩溃留下的陈旧锁（超过 20 分钟）不算数
        if l.exists() && std::fs::metadata(l).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|e| e > Duration::from_secs(1200)) {
            let _ = std::fs::remove_file(l);
        }
        if let Some(d) = l.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        match std::fs::OpenOptions::new().write(true).create_new(true).open(l) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                log("已有另一个更新程序在运行，本次退出");
                return Ok(());
            }
            Err(e) => bail!("无法创建更新锁 {}：{e}", l.display()),
        }
    }
    let result = apply_locked(a);
    if let Some(l) = &lock {
        let _ = std::fs::remove_file(l);
    }
    if let Ok(me) = std::env::current_exe() {
        if me.file_name().is_some_and(|n| n.to_string_lossy().starts_with("updater-")) {
            let _ = std::fs::remove_file(me);
        }
    }
    match &result {
        Ok(()) => {}
        Err(e) => {
            log(&format!("更新到 {} 失败，已保留当前版本：{e:#}", a.version));
            write_outcome(&a.version, false, format!("{e:#}"));
        }
    }
    result
}

fn apply_locked(a: &ApplyArgs) -> Result<()> {
    log(&format!("开始更新到 {}：等待 {} 退出", a.version, a.app.display()));
    let deadline = Instant::now() + Duration::from_secs(a.wait_secs.max(1));
    loop {
        if !pid_alive(a.pid) && running_under(&a.app).is_empty() {
            break;
        }
        if Instant::now() >= deadline {
            bail!("等待 {} 秒后 App 仍在运行，本次不更新（下次退出时会再试）", a.wait_secs);
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    if !a.staged.join("Contents/Info.plist").is_file() {
        bail!("暂存的新版本不见了：{}", a.staged.display());
    }
    // 暂存之后到现在可能被改动：替换前再验一遍
    verify_bundle(&a.staged, &a.version, &a.app).context("替换前复检新版本不通过")?;
    let old_team = team_of(&a.app).ok();

    // 原子交换；文件系统不支持时退回两步改名（失败即还原）
    let mut leftover = a.staged.clone();
    match swap(&a.staged, &a.app) {
        Ok(()) => log("已原子交换新旧 App"),
        Err(e) => {
            log(&format!("原子交换不可用（{e}），改用两步改名"));
            let old = a.app.with_file_name(format!(".requestbaby-old-{}.app", std::process::id()));
            std::fs::rename(&a.app, &old).context("移开旧版本失败")?;
            if let Err(e) = std::fs::rename(&a.staged, &a.app) {
                let _ = std::fs::rename(&old, &a.app);
                bail!("放入新版本失败，已还原旧版本：{e}");
            }
            leftover = old;
        }
    }
    // 复检装好的新 App；不对就换回
    let check = (|| -> Result<()> {
        verify_bundle(&a.app, &a.version, &leftover)?;
        if let Some(t) = &old_team {
            if &team_of(&a.app)? != t {
                bail!("装好的新版本签名团队不一致");
            }
        }
        Ok(())
    })();
    if let Err(e) = check {
        let rolled = swap(&leftover, &a.app).or_else(|_| {
            let broken = a.app.with_file_name(format!(".requestbaby-broken-{}.app", std::process::id()));
            std::fs::rename(&a.app, &broken).and_then(|_| std::fs::rename(&leftover, &a.app)).inspect(|_| {
                let _ = std::fs::remove_dir_all(&broken);
            })
        });
        match rolled {
            Ok(()) => {
                let _ = std::fs::remove_dir_all(&leftover);
                bail!("新版本装好后复检不通过，已换回旧版本：{e:#}");
            }
            Err(r) => bail!("新版本复检不通过（{e:#}），且还原旧版本失败：{r}；旧版本留在 {}", leftover.display()),
        }
    }
    let _ = std::fs::remove_dir_all(&leftover);
    clear_pending();
    write_outcome(&a.version, true, format!("已更新到 {}", a.version));
    log(&format!("已更新到 {}", a.version));
    if a.relaunch {
        let _ = Command::new("/usr/bin/open").arg(&a.app).stdin(Stdio::null()).status();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_sits_next_to_the_installed_app_on_the_same_volume() {
        let p = staging_path(Path::new("/Applications/requestbaby.app"), "0.4.0");
        assert_eq!(p, Path::new("/Applications/.requestbaby-update-0.4.0.app"));
    }

    #[test]
    fn translocated_or_dmg_apps_do_not_self_update() {
        let t = self_update_blocker(Path::new("/private/var/folders/x/AppTranslocation/ABC/d/requestbaby.app"));
        assert!(t.is_some_and(|m| m.contains("拖进")));
        assert!(self_update_blocker(Path::new("/Volumes/requestbaby/requestbaby.app")).is_some());
    }

    #[test]
    fn unwritable_install_dir_is_reported_before_touching_anything() {
        let why = self_update_blocker(Path::new("/System/Applications/requestbaby.app")).unwrap();
        assert!(why.contains("没有权限"), "{why}");
    }

    #[test]
    fn only_processes_inside_the_app_count_as_running() {
        // 本测试进程不在任何 .app 里
        assert!(running_under(Path::new("/Applications/definitely-not-installed.app")).is_empty());
        assert!(pid_alive(std::process::id()));
    }
}
