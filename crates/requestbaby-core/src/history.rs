//! 运行历史：`<项目根>/.requestbaby/runs/<id>.json`
//!
//! CLI 与 Desktop 写入同一目录、同一协议（[`RunReport`]），
//! AI 在终端跑过的每一次运行，人都能在 Desktop 的 History 面板看到。
use crate::report::{CaseRef, Comparison, RunReport};
use std::collections::BTreeMap;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 历史保留条数（超出按时间淘汰最旧）
pub const KEEP_RUNS: usize = 200;
/// 截图等证据保留：最近这么多次运行全部保留；更早的只保留有失败的（通过的证据删掉，记录本身仍在）
pub const KEEP_ARTIFACTS: usize = 30;
/// 单发（send）的证据只保留最近这么多次
pub const KEEP_SCRATCH: usize = 20;

/// 单发的临时证据目录
pub fn scratch_dir(root: &Path) -> PathBuf {
    state_dir(root).join("scratch")
}

/// 本次运行的证据目录：写历史的运行放在 `runs/<运行 ID>/`，单发放在 `scratch/<运行 ID>/`
pub fn artifacts_for(root: &Path, id: &str, recorded: bool) -> crate::runner::ArtifactsDir {
    let base = if recorded { runs_dir(root) } else { scratch_dir(root) };
    if !recorded {
        prune_dirs(&base, KEEP_SCRATCH.saturating_sub(1));
    }
    crate::runner::ArtifactsDir { root: root.to_path_buf(), dir: base.join(id) }
}

/// 运行时的代码版本（结果绑定到提交：哪个版本通过 / 失败一目了然）；不是 git 仓库或没装 git 时为 None
pub fn git_info(root: &Path) -> Option<crate::report::GitInfo> {
    let git = |args: &[&str]| -> Option<String> {
        let out = std::process::Command::new("git").arg("-C").arg(root).args(args).stderr(std::process::Stdio::null()).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let commit = git(&["rev-parse", "HEAD"])?;
    let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
    // 只看已跟踪文件的改动：.requestbaby/ 等未跟踪的本地状态不算「改过代码」
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
    Some(crate::report::GitInfo { commit, branch: if branch == "HEAD" { String::new() } else { branch }, dirty })
}

pub fn state_dir(root: &Path) -> PathBuf {
    root.join(".requestbaby")
}

pub fn runs_dir(root: &Path) -> PathBuf {
    state_dir(root).join("runs")
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// 运行 ID：时间戳在前保证文件名序 = 时间序
pub fn new_run_id(source: &str, started_at_ms: u64) -> String {
    format!("{started_at_ms}-{source}-{}", std::process::id())
}

/// 写入一条运行记录，返回文件路径
pub fn save(root: &Path, report: &RunReport) -> Result<PathBuf> {
    let dir = runs_dir(root);
    std::fs::create_dir_all(&dir).with_context(|| format!("创建 {} 失败", dir.display()))?;
    let id = report.meta.as_ref().map(|m| m.id.clone()).unwrap_or_else(|| new_run_id("unknown", now_ms()));
    let path = dir.join(format!("{id}.json"));
    crate::fsx::write(&path, serde_json::to_vec_pretty(report)?)?;
    prune(&dir, KEEP_RUNS);
    prune_artifacts(&dir, KEEP_ARTIFACTS);
    Ok(path)
}

/// 历史条目（列表展示用，不含用例明细）
#[derive(Debug, Clone)]
pub struct RunEntry {
    pub path: PathBuf,
    pub report: RunReport,
}

/// 读取全部运行记录（新 → 旧）；损坏文件跳过
pub fn list(root: &Path) -> Vec<RunEntry> {
    list_paths(root)
        .into_iter()
        .filter_map(|path| load(&path).ok().map(|report| RunEntry { path, report }))
        .collect()
}

/// 删除一条运行记录（按运行 ID）
pub fn delete(root: &Path, id: &str) -> Result<()> {
    let path = runs_dir(root).join(format!("{id}.json"));
    if id.contains(['/', '\\']) || !path.is_file() {
        anyhow::bail!("运行记录不存在: {id}（requestbaby history 查看 ID）");
    }
    std::fs::remove_file(&path).with_context(|| format!("删除失败: {}", path.display()))?;
    remove_artifacts(&path);
    Ok(())
}

/// 删除运行记录对应的证据目录（`<运行 ID>/`）
fn remove_artifacts(json: &Path) {
    let dir = json.with_extension("");
    if dir.is_dir() {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// 证据保留：最近 keep 次全部保留；更早的只保留有失败的；记录已删除的证据目录一并清掉
fn prune_artifacts(dir: &Path, keep: usize) {
    let mut ids: Vec<String> = run_files(dir).iter().filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned())).collect();
    ids.sort();
    ids.reverse();
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        let id = e.file_name().to_string_lossy().into_owned();
        match ids.iter().position(|x| *x == id) {
            None => {
                // 运行中的证据目录还没有记录文件：只清理一小时前的残留
                let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|d| d.as_secs() > 3600);
                if old {
                    let _ = std::fs::remove_dir_all(&p);
                }
            }
            Some(i) if i >= keep => {
                let failed = load(&dir.join(format!("{id}.json"))).map(|r| r.failed > 0 || r.blocked.is_some()).unwrap_or(false);
                if !failed {
                    let _ = std::fs::remove_dir_all(&p);
                }
            }
            _ => {}
        }
    }
}

/// 只保留最新的 keep 个子目录（单发证据）
fn prune_dirs(dir: &Path, keep: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    if dirs.len() <= keep {
        return;
    }
    dirs.sort();
    for d in &dirs[..dirs.len() - keep] {
        let _ = std::fs::remove_dir_all(d);
    }
}

/// 批量清理条件（都给出时取交集：最近 keep 条之外、且早于 older_than_ms 的才删）
#[derive(Debug, Clone, Copy, Default)]
pub struct Clean {
    /// 保留最近的条数（None = 不按条数保留）
    pub keep: Option<usize>,
    /// 只删开始时间早于「现在 - 该毫秒数」的
    pub older_than_ms: Option<u64>,
}

/// 批量清理运行历史，返回删除条数。`Clean::default()` = 全部清空
pub fn clean(root: &Path, c: Clean) -> Result<usize> {
    let mut files = run_files(&runs_dir(root));
    files.sort();
    files.reverse();
    let cutoff = c.older_than_ms.map(|d| now_ms().saturating_sub(d));
    let mut removed = 0;
    for f in files.iter().skip(c.keep.unwrap_or(0)) {
        let started = f.file_stem().and_then(|s| s.to_str()).and_then(|s| s.split('-').next()).and_then(|t| t.parse::<u64>().ok());
        if cutoff.is_some_and(|cut| started.is_none_or(|t| t >= cut)) {
            continue;
        }
        std::fs::remove_file(f).with_context(|| format!("删除失败: {}", f.display()))?;
        remove_artifacts(f);
        removed += 1;
    }
    Ok(removed)
}

/// 解析时长：`30m` / `12h` / `7d`（纯数字按天）
pub fn parse_duration_ms(s: &str) -> Result<u64> {
    let s = s.trim();
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let n: u64 = num.parse().with_context(|| format!("无法解析时长: {s}（例如 30m / 12h / 7d）"))?;
    let unit_ms = match unit {
        "" | "d" => 86_400_000,
        "h" => 3_600_000,
        "m" => 60_000,
        _ => anyhow::bail!("无法解析时长: {s}（单位 m / h / d）"),
    };
    Ok(n * unit_ms)
}

/// 运行记录文件，新 → 旧（只列路径不解析：常驻进程据此增量加载）
pub fn list_paths(root: &Path) -> Vec<PathBuf> {
    let mut files = run_files(&runs_dir(root));
    files.sort();
    files.reverse();
    files
}

fn run_files(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect())
        .unwrap_or_default()
}

pub fn load(path: &Path) -> Result<RunReport> {
    let text = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
}

/// 每个用例在历史中最近一次的结论（同环境；dry-run 与跳过的不算）：true = 通过
pub fn last_status(root: &Path, env: &str) -> BTreeMap<CaseRef, bool> {
    let mut out = BTreeMap::new();
    for e in list(root) {
        let Some(meta) = &e.report.meta else { continue };
        if meta.dry_run || meta.env != env {
            continue;
        }
        for r in e.report.results.iter().filter(|r| !r.skipped) {
            out.entry(CaseRef { file: r.file.clone(), name: r.name.clone() }).or_insert(r.passed);
        }
    }
    out
}

/// 本次结果相对基准的变化；基准为空（从未运行过）时 None
pub fn compare(baseline: &BTreeMap<CaseRef, bool>, report: &RunReport) -> Option<Comparison> {
    if baseline.is_empty() {
        return None;
    }
    let mut c = Comparison::default();
    for r in report.results.iter().filter(|r| !r.skipped) {
        let key = CaseRef { file: r.file.clone(), name: r.name.clone() };
        // verify 中同一用例可能执行多次（全部用例 + 流程步骤）：只记一次
        if c.fixed.contains(&key) || c.still_failing.contains(&key) || c.new_failures.contains(&key) {
            continue;
        }
        match (baseline.get(&key), r.passed) {
            (Some(false), true) => c.fixed.push(key),
            (Some(false), false) => c.still_failing.push(key),
            (Some(true) | None, false) => c.new_failures.push(key),
            _ => {}
        }
    }
    Some(c)
}

/// 最近一次结论为失败的用例文件（相对项目根，执行序；文件已删除的跳过）
pub fn failed_files(root: &Path, env: &str) -> Vec<String> {
    let mut files: Vec<String> = last_status(root, env).into_iter().filter(|(_, ok)| !ok).map(|(k, _)| k.file).collect();
    files.dedup();
    files.retain(|f| root.join(f).is_file());
    files
}

fn prune(dir: &Path, keep: usize) {
    let mut files = run_files(dir);
    if files.len() <= keep {
        return;
    }
    files.sort();
    for old in &files[..files.len() - keep] {
        let _ = std::fs::remove_file(old);
        remove_artifacts(old);
    }
}

/// 不稳定的用例：同一提交（代码没改过）上既通过过又失败过——结论不可信，需要人看。
/// 看本次与最近 20 次同环境运行；没有 git 信息或工作区有改动的运行不参与判断
pub fn flaky(root: &Path, env: &str, current: &RunReport) -> Vec<CaseRef> {
    let mut seen: BTreeMap<(CaseRef, String), (bool, bool)> = BTreeMap::new();
    let mut add = |report: &RunReport| {
        let Some(meta) = &report.meta else { return };
        let Some(git) = meta.git.as_ref().filter(|g| !g.dirty) else { return };
        if meta.dry_run || meta.env != env {
            return;
        }
        for r in report.results.iter().filter(|r| !r.skipped) {
            let e = seen.entry((CaseRef { file: r.file.clone(), name: r.name.clone() }, git.commit.clone())).or_default();
            if r.passed {
                e.0 = true;
            } else {
                e.1 = true;
            }
        }
    };
    add(current);
    let current_id = current.meta.as_ref().map(|m| m.id.clone()).unwrap_or_default();
    for path in list_paths(root).into_iter().take(21) {
        if path.file_stem().is_some_and(|s| s.to_string_lossy() == current_id) {
            continue;
        }
        if let Ok(r) = load(&path) {
            add(&r);
        }
    }
    let mut out: Vec<CaseRef> = seen.into_iter().filter(|(_, (p, f))| *p && *f).map(|((c, _), _)| c).collect();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::RunMeta;

    fn save_run(root: &Path, started_at_ms: u64) -> String {
        let id = new_run_id("cli", started_at_ms);
        let id = format!("{id}-{started_at_ms}");
        let meta = RunMeta { id: id.clone(), started_at_ms, ..Default::default() };
        save(root, &RunReport::from_cases(vec![], Some(meta))).unwrap();
        id
    }

    #[test]
    fn delete_and_clean() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let now = now_ms();
        let day = 86_400_000;
        let ids: Vec<String> = [10, 8, 3, 1, 0].iter().map(|d| save_run(root, now - d * day - 1000)).collect();
        assert_eq!(list(root).len(), 5);

        delete(root, &ids[4]).unwrap();
        assert!(delete(root, &ids[4]).unwrap_err().to_string().contains("不存在"));
        assert!(delete(root, "../x").is_err());

        // 7 天前的：10 天、8 天前两条
        assert_eq!(clean(root, Clean { older_than_ms: Some(7 * day), ..Default::default() }).unwrap(), 2);
        // 只保留最近 1 条
        assert_eq!(clean(root, Clean { keep: Some(1), ..Default::default() }).unwrap(), 1);
        assert_eq!(list(root)[0].report.meta.as_ref().unwrap().id, ids[3]);
        // 全部清空
        assert_eq!(clean(root, Clean::default()).unwrap(), 1);
        assert!(list(root).is_empty());

        assert_eq!(parse_duration_ms("7d").unwrap(), 7 * day);
        assert_eq!(parse_duration_ms("12h").unwrap(), 12 * 3_600_000);
        assert_eq!(parse_duration_ms("3").unwrap(), 3 * day);
        assert!(parse_duration_ms("7w").is_err());
    }
}
