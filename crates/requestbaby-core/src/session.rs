//! 一次完整运行：发现 → 加载上下文 → 执行 → 生成报告 → 写历史 → 事件。
//!
//! CLI 与 Desktop 的「运行」「单发」都从 [`RunRequest`] / [`send_spec`] 构造 [`SessionSpec`]，
//! 再进入同一个 [`run`]：参数语义、等价命令行、报告与历史协议两端完全一致。
use crate::config::{load_strict, Project};
use crate::history;
use crate::report::{RunMeta, RunReport};
use crate::runner::{self, discover, CancelToken, CaseResult, EventHook, RunEvent, RunOptions, Runner};
use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

/// 一次「运行」的参数（与 `requestbaby run` 的参数一一对应）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunRequest {
    /// 文件 / 目录（相对 base；空 = collections）
    pub paths: Vec<String>,
    pub env: Option<String>,
    /// 标签过滤（可含逗号分隔，任一命中即选）
    pub tags: Vec<String>,
    pub name: Option<String>,
    pub fail_fast: bool,
    pub dry_run: bool,
    pub verbose: bool,
    pub parallel: Option<usize>,
    /// 响应落盘目录（相对 base）
    pub output: Option<String>,
}

impl RunRequest {
    pub fn paths_or_default(&self) -> Vec<String> {
        if self.paths.is_empty() {
            vec!["collections".into()]
        } else {
            self.paths.clone()
        }
    }

    /// 规范化标签：拆分逗号、去空白
    pub fn normalized_tags(&self) -> Vec<String> {
        self.tags
            .iter()
            .flat_map(|t| t.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()))
            .collect()
    }

    /// 等价 CLI 命令（写入历史；Desktop 展示「复制为命令」）
    pub fn command_line(&self) -> String {
        let mut parts = vec!["requestbaby run".to_string()];
        parts.extend(self.paths_or_default().into_iter().map(|p| quote(&p)));
        if let Some(e) = &self.env {
            parts.push(format!("-e {}", quote(e)));
        }
        let tags = self.normalized_tags();
        if !tags.is_empty() {
            parts.push(format!("--tag {}", quote(&tags.join(","))));
        }
        if let Some(n) = self.name.as_ref().filter(|n| !n.is_empty()) {
            parts.push(format!("--name {}", quote(n)));
        }
        if self.fail_fast {
            parts.push("--fail-fast".into());
        }
        if self.dry_run {
            parts.push("--dry-run".into());
        }
        if let Some(p) = self.parallel {
            parts.push(format!("--parallel {p}"));
        }
        if let Some(o) = self.output.as_ref().filter(|o| !o.is_empty()) {
            parts.push(format!("-o {}", quote(o)));
        }
        parts.join(" ")
    }

    /// 构造执行规格。`base`：相对路径的基准目录（CLI 为当前目录，Desktop 为项目根）
    pub fn to_spec(&self, base: &Path, source: &str, send_mode: bool, cancel: Option<CancelToken>) -> SessionSpec {
        let resolve = |p: &str| {
            let pb = PathBuf::from(p);
            if pb.is_absolute() {
                pb
            } else {
                base.join(pb)
            }
        };
        SessionSpec {
            paths: self.paths_or_default().iter().map(|p| resolve(p)).collect(),
            env: self.env.clone(),
            opts: RunOptions {
                tags: self.normalized_tags(),
                name_filter: self.name.clone().unwrap_or_default(),
                fail_fast: self.fail_fast,
                dry_run: self.dry_run,
                verbose: self.verbose,
                output_dir: self.output.as_ref().filter(|o| !o.is_empty()).map(|o| resolve(o)),
                send_mode,
                parallel: self.parallel,
                cancel,
            },
            source: source.to_string(),
            command: self.command_line(),
            record_history: true,
        }
    }
}

/// 单发调试（`requestbaby send` / Desktop「发送」）：保留完整响应，不写历史
pub fn send_spec(case: PathBuf, display: &str, env: Option<String>, source: &str, cancel: Option<CancelToken>) -> SessionSpec {
    let mut command = format!("requestbaby send {}", quote(display));
    if let Some(e) = &env {
        command.push_str(&format!(" -e {}", quote(e)));
    }
    SessionSpec {
        paths: vec![case],
        env,
        opts: RunOptions { send_mode: true, cancel, ..Default::default() },
        source: source.to_string(),
        command,
        record_history: false,
    }
}

/// 命令行参数含空白时加引号
pub fn quote(s: &str) -> String {
    if s.is_empty() || s.chars().any(|c| c.is_whitespace() || c == '"' || c == '\'') {
        format!("'{}'", s.replace('\'', "'\\''"))
    } else {
        s.to_string()
    }
}

pub struct SessionSpec {
    /// 文件 / 目录（绝对或相对当前目录）
    pub paths: Vec<PathBuf>,
    /// 环境名；None 时取项目 defaultEnv，再缺省 dev
    pub env: Option<String>,
    pub opts: RunOptions,
    /// cli / desktop / ci
    pub source: String,
    /// 等价 CLI 命令（写入历史，便于复现）
    pub command: String,
    /// 是否写入 `.requestbaby/runs/`（dry-run 永不写）
    pub record_history: bool,
}

pub struct SessionOutcome {
    pub env: String,
    pub planned: usize,
    pub timeout_ms: u64,
    pub results: Vec<CaseResult>,
    pub report: RunReport,
    pub notices: Vec<String>,
    pub history_path: Option<PathBuf>,
}

/// 从第一个路径定位项目默认环境
pub fn default_env_of(paths: &[PathBuf]) -> Option<String> {
    let root = discover::find_project_root(paths.first()?)?;
    let p = discover::project_file(&root).and_then(|pf| load_strict::<Project>(&pf).ok())?;
    Some(p.default_env)
}

pub fn resolve_env(paths: &[PathBuf], env: Option<&str>) -> String {
    env.map(String::from).or_else(|| default_env_of(paths)).unwrap_or_else(|| "dev".into())
}

/// 执行一次运行。`on_start` 在执行前回调（CLI 打印表头用），`hook` 接收全部事件。
pub fn run(spec: &SessionSpec, hook: Option<EventHook>, on_start: impl FnOnce(&str, usize, u64)) -> Result<SessionOutcome> {
    if let Some(missing) = spec.paths.iter().find(|p| !p.exists()) {
        bail!("路径不存在: {}", missing.display());
    }
    let cases = discover::discover(&spec.paths)?;
    if cases.is_empty() {
        bail!("未发现用例（*.json，排除 requestbaby.json/collection.json/envs）");
    }
    let env = resolve_env(&spec.paths, spec.env.as_deref());
    let ctx = runner::load_project_ctx(&spec.paths[0], &env)?;
    let timeout_ms = ctx.project.timeout_ms;
    let started_at_ms = history::now_ms();
    let id = history::new_run_id(&spec.source, started_at_ms);
    let emit = |ev: RunEvent| {
        if let Some(h) = &hook {
            h(&ev);
        }
    };

    on_start(&env, cases.len(), timeout_ms);
    emit(RunEvent::RunStarted { id: id.clone(), env: env.clone(), source: spec.source.clone(), planned: cases.len() });

    let mut r = Runner::new(ctx);
    if let Some(h) = &hook {
        r.set_hook(h.clone());
    }
    let results = r.run(&cases, &spec.opts, |_| {})?;
    let notices = r.take_notices();

    let meta = RunMeta {
        id,
        source: spec.source.clone(),
        command: spec.command.clone(),
        env: env.clone(),
        started_at_ms,
        finished_at_ms: history::now_ms(),
        dry_run: spec.opts.dry_run,
        cancelled: spec.opts.cancel.as_ref().is_some_and(|c| c.is_cancelled()),
        workflow: None,
    };
    let report = RunReport::new(&results, true, Some(meta));
    let history_path = if spec.record_history && !spec.opts.dry_run {
        discover::find_project_root(&spec.paths[0]).and_then(|root| history::save(&root, &report).ok())
    } else {
        None
    };
    emit(RunEvent::RunFinished { report: report.clone() });
    Ok(SessionOutcome { env, planned: cases.len(), timeout_ms, results, report, notices, history_path })
}

/// 项目根（路径向上查找 requestbaby.json）
pub fn project_root(start: &Path) -> Option<PathBuf> {
    discover::find_project_root(start)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_line_round_trips_options() {
        let r = RunRequest {
            paths: vec!["collections/core".into()],
            env: Some("dev".into()),
            tags: vec!["smoke, auth".into()],
            name: Some("login".into()),
            fail_fast: true,
            parallel: Some(4),
            output: Some("out dir".into()),
            ..Default::default()
        };
        assert_eq!(
            r.command_line(),
            "requestbaby run collections/core -e dev --tag smoke,auth --name login --fail-fast --parallel 4 -o 'out dir'"
        );
        assert_eq!(RunRequest::default().command_line(), "requestbaby run collections");
    }
}
