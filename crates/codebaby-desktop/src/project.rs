//! 项目模型：只读写项目目录（与 CLI 同一份文件），不持有任何私有状态。
//!
//! - 用例树：`collections/` 目录即集合
//! - 环境：`envs/*.json`
//! - 运行历史：`.codebaby/runs/*.json`（CLI / Desktop 共享）
use codebaby_core::config::{load_strict, Project as ProjectConfig, TestCase};
use codebaby_core::history;
use codebaby_core::report::{CaseReport, RunReport};
use codebaby_core::runner::discover;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 单个用例的静态信息（来自文件）
#[derive(Debug, Clone)]
pub struct CaseInfo {
    /// 相对项目根，与运行结果 `file` 字段一致
    pub rel: String,
    pub path: PathBuf,
    pub name: String,
    pub method: String,
    /// 原始 URL 模板（搜索用）
    pub url: String,
    pub parse_error: Option<String>,
}

/// 最近一次运行中某用例的聚合状态（数据驱动多实例：任一失败即失败）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseStatus {
    Passed,
    Failed,
    Skipped,
}

pub struct Project {
    root: PathBuf,
    config: ProjectConfig,
    envs: Vec<String>,
    cases: Vec<CaseInfo>,
    history: Vec<RunReport>,
    /// 与 history 一一对应的文件（运行记录写入后不再改动：重载时只解析新文件）
    history_paths: Vec<(PathBuf, u64)>,
}

impl Project {
    pub fn load(root: PathBuf) -> Self {
        let mut project = Self {
            root,
            config: ProjectConfig::default(),
            envs: vec![],
            cases: vec![],
            history: vec![],
            history_paths: vec![],
        };
        project.reload_files();
        project.reload_history();
        project
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn name(&self) -> String {
        self.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "project".into())
    }

    pub fn config(&self) -> &ProjectConfig {
        &self.config
    }

    pub fn envs(&self) -> &[String] {
        &self.envs
    }

    pub fn cases(&self) -> &[CaseInfo] {
        &self.cases
    }

    pub fn case(&self, rel: &str) -> Option<&CaseInfo> {
        self.cases.iter().find(|c| c.rel == rel)
    }

    pub fn history(&self) -> &[RunReport] {
        &self.history
    }

    pub fn collections_dir(&self) -> PathBuf {
        self.root.join("collections")
    }

    pub fn project_file(&self) -> PathBuf {
        self.root.join(discover::PROJECT_FILE)
    }

    /// 相对项目根的路径（展示 / 等价命令用）
    pub fn rel_of(&self, path: &Path) -> String {
        path.strip_prefix(&self.root).unwrap_or(path).to_string_lossy().replace('\\', "/")
    }

    /// collections/ 下全部集合目录（相对项目根，含 collections 自身）
    pub fn collection_dirs(&self) -> Vec<String> {
        let mut dirs = vec!["collections".to_string()];
        for e in walkdir::WalkDir::new(self.collections_dir()).min_depth(1).into_iter().flatten() {
            if e.file_type().is_dir() && !e.file_name().to_string_lossy().starts_with('.') {
                dirs.push(self.rel_of(e.path()));
            }
        }
        dirs.sort();
        dirs
    }

    /// 重新读取配置、环境与用例（文件变更后调用）
    pub fn reload_files(&mut self) {
        self.config = discover::project_file(&self.root)
            .and_then(|p| load_strict::<ProjectConfig>(&p).ok())
            .unwrap_or_default();
        let envs = codebaby_core::config::list_envs(&self.root);
        self.envs = envs;
        self.cases = discover::discover(&[self.collections_dir()])
            .unwrap_or_default()
            .into_iter()
            .map(|dc| case_info(dc.rel, dc.path))
            .collect();
    }

    pub fn reload_history(&mut self) {
        let mut known: HashMap<(PathBuf, u64), RunReport> =
            std::mem::take(&mut self.history_paths).into_iter().zip(std::mem::take(&mut self.history)).collect();
        for path in history::list_paths(&self.root) {
            let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let key = (path, len);
            let report = match known.remove(&key) {
                Some(r) => r,
                None => match history::load(&key.0) {
                    Ok(r) => r,
                    Err(_) => continue,
                },
            };
            self.history_paths.push(key);
            self.history.push(report);
        }
    }

    /// 最近一次覆盖到该用例的运行结果
    pub fn latest_status(&self) -> HashMap<String, CaseStatus> {
        let mut out: HashMap<String, CaseStatus> = HashMap::new();
        // history 新 → 旧：首次见到的运行即最近结果
        let mut seen_in_run: HashMap<String, usize> = HashMap::new();
        for (ix, run) in self.history.iter().enumerate() {
            for r in &run.results {
                if let Some(&first_ix) = seen_in_run.get(&r.file) {
                    if first_ix != ix {
                        continue;
                    }
                }
                seen_in_run.insert(r.file.clone(), ix);
                let st = status_of(r);
                out.entry(r.file.clone())
                    .and_modify(|s| {
                        if st == CaseStatus::Failed {
                            *s = CaseStatus::Failed;
                        }
                    })
                    .or_insert(st);
            }
        }
        out
    }
}

pub fn status_of(r: &CaseReport) -> CaseStatus {
    if r.skipped {
        CaseStatus::Skipped
    } else if r.passed {
        CaseStatus::Passed
    } else {
        CaseStatus::Failed
    }
}

fn case_info(rel: String, path: PathBuf) -> CaseInfo {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    match load_strict::<TestCase>(&path) {
        Ok(c) => CaseInfo {
            rel,
            name: if c.name.is_empty() { stem } else { c.name.clone() },
            method: c.summary().0,
            url: c.summary().1,
            parse_error: None,
            path,
        },
        Err(e) => CaseInfo { rel, path, name: stem, method: "-".into(), url: String::new(), parse_error: Some(format!("{e:#}")) },
    }
}
