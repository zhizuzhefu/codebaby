//! 运行历史：`<项目根>/.requestbaby/runs/<id>.json`
//!
//! CLI 与 Desktop 写入同一目录、同一协议（[`RunReport`]），
//! AI 在终端跑过的每一次运行，人都能在 Desktop 的 History 面板看到。
use crate::report::RunReport;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 历史保留条数（超出按时间淘汰最旧）
pub const KEEP_RUNS: usize = 200;

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
    std::fs::write(&path, serde_json::to_vec_pretty(report)?)?;
    prune(&dir, KEEP_RUNS);
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
    let mut files: Vec<PathBuf> = std::fs::read_dir(runs_dir(root))
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect())
        .unwrap_or_default();
    files.sort();
    files.reverse();
    files
        .into_iter()
        .filter_map(|path| load(&path).ok().map(|report| RunEntry { path, report }))
        .collect()
}

pub fn load(path: &Path) -> Result<RunReport> {
    let text = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
}

fn prune(dir: &Path, keep: usize) {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect())
        .unwrap_or_default();
    if files.len() <= keep {
        return;
    }
    files.sort();
    for old in &files[..files.len() - keep] {
        let _ = std::fs::remove_file(old);
    }
}
