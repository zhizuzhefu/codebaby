//! 环境自检：`requestbaby doctor` 与 Desktop「环境自检」共用。
use crate::config::{load_strict, EnvFile, Project};
use crate::runner::discover;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckLevel {
    Ok,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub level: CheckLevel,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorReport {
    pub root: Option<PathBuf>,
    pub checks: Vec<Check>,
}

impl DoctorReport {
    pub fn ok(&self) -> bool {
        self.checks.iter().all(|c| c.level != CheckLevel::Error)
    }
}

pub fn run(start: &Path) -> DoctorReport {
    let mut checks = vec![Check {
        level: CheckLevel::Ok,
        message: format!("HTTP 引擎 libcurl {}（静态链接，无需系统 curl）", crate::engine::libcurl_version()),
    }];
    let push = |checks: &mut Vec<Check>, level, message: String| checks.push(Check { level, message });

    let root = discover::find_project_root(start);
    match &root {
        None => push(&mut checks, CheckLevel::Warn, "当前不在项目内（无 requestbaby.json）".into()),
        Some(root) => {
            push(&mut checks, CheckLevel::Ok, format!("项目根: {}", root.display()));
            match discover::project_file(root).map(|pf| load_strict::<Project>(&pf)) {
                Some(Ok(p)) => {
                    push(
                        &mut checks,
                        CheckLevel::Ok,
                        format!("项目配置 ok（defaultEnv={} workflows={}）", p.default_env, p.workflows.len()),
                    );
                    if !root.join("envs").join(format!("{}.json", p.default_env)).exists() {
                        push(&mut checks, CheckLevel::Warn, format!("默认环境 {} 没有对应的 envs/{}.json", p.default_env, p.default_env));
                    }
                }
                Some(Err(e)) => push(&mut checks, CheckLevel::Error, format!("项目配置解析失败: {e:#}")),
                None => {}
            }
            let envs_dir = root.join("envs");
            if envs_dir.is_dir() {
                let mut names = vec![];
                for e in std::fs::read_dir(&envs_dir).into_iter().flatten().flatten() {
                    let p = e.path();
                    if p.extension().is_some_and(|x| x == "json") {
                        match load_strict::<EnvFile>(&p) {
                            Ok(_) => names.push(p.file_stem().unwrap_or_default().to_string_lossy().into_owned()),
                            Err(err) => push(&mut checks, CheckLevel::Error, format!("环境文件 {} 解析失败: {err:#}", p.display())),
                        }
                    }
                }
                names.sort();
                push(&mut checks, CheckLevel::Ok, format!("环境文件 {} 个（{}）", names.len(), names.join(", ")));
            } else {
                push(&mut checks, CheckLevel::Warn, "无 envs/ 目录".into());
            }
            match discover::discover(&[root.join("collections")]) {
                Ok(cases) => push(&mut checks, CheckLevel::Ok, format!("用例 {} 个", cases.len())),
                Err(e) => push(&mut checks, CheckLevel::Warn, format!("collections/ 不可读: {e:#}")),
            }
            let runs = crate::history::list(root).len();
            push(&mut checks, CheckLevel::Ok, format!("运行历史 {runs} 条（{}）", crate::history::runs_dir(root).display()));
        }
    }
    DoctorReport { root, checks }
}
