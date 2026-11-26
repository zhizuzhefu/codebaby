//! 一次完整运行：发现 → 加载上下文 → 执行 → 生成报告 → 写历史 → 事件。
//!
//! CLI 与 Desktop 的「运行」「单发」都从 [`RunRequest`] / [`send_spec`] 构造 [`SessionSpec`]，
//! 再进入同一个 [`run`]：参数语义、等价命令行、报告与历史协议两端完全一致。
use crate::config::{load_strict, Project};
use crate::history;
use crate::report::{RunMeta, RunReport};
use crate::runner::{self, discover, CancelToken, CaseResult, EventHook, RunEvent, RunOptions, Runner, SessionState};
use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

/// 一次「运行」的参数（与 `codebaby run` 的参数一一对应）
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
    /// 只重跑最近失败的用例（含其依赖的前置用例），范围仍受 paths / 过滤条件限制
    pub failed: bool,
    /// Web 用例显示浏览器窗口
    pub headed: bool,
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
        let mut parts = vec!["codebaby run".to_string()];
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
        if self.failed {
            parts.push("--failed".into());
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
        if self.headed {
            parts.push("--headed".into());
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
                artifacts: None,
                headed: self.headed,
            },
            source: source.to_string(),
            command: self.command_line(),
            record_history: true,
            seed: None,
            only_failed: self.failed,
            prerequisites: false,
        }
    }
}

/// 单发调试（`codebaby send` / Desktop「发送」）：保留完整响应，不写历史
pub fn send_spec(case: PathBuf, display: &str, env: Option<String>, source: &str, cancel: Option<CancelToken>) -> SessionSpec {
    let mut command = format!("codebaby send {}", quote(display));
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
        seed: None,
        only_failed: false,
        prerequisites: true,
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
    /// 是否写入 `.codebaby/runs/`（dry-run 永不写）
    pub record_history: bool,
    /// 以已有会话状态开始（Desktop 单发：之前捕获的变量与 Cookie）
    pub seed: Option<SessionState>,
    /// 只跑最近失败的用例（+ 前置依赖）
    pub only_failed: bool,
    /// 单发：用到的运行时变量（如 token）会话里还没有时，先运行捕获它的前置用例
    pub prerequisites: bool,
}

pub struct SessionOutcome {
    pub env: String,
    pub planned: usize,
    pub timeout_ms: u64,
    pub results: Vec<CaseResult>,
    pub report: RunReport,
    pub notices: Vec<String>,
    pub history_path: Option<PathBuf>,
    /// 运行结束时的会话状态（捕获的变量 + Cookie）
    pub state: SessionState,
    /// 单发前自动补跑的前置用例（相对项目根）；results 里排在目标用例之前
    pub prerequisites: Vec<String>,
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
    let mut cases = discover::discover(&spec.paths)?;
    if cases.is_empty() {
        bail!("未发现用例（*.json，排除 codebaby.json/collection.json/envs）");
    }
    let env = resolve_env(&spec.paths, spec.env.as_deref());
    let root = discover::find_project_root(&spec.paths[0]);
    if spec.only_failed {
        let Some(root) = &root else { bail!("--failed 需要在项目内运行（未找到 codebaby.json）") };
        let failed = history::failed_files(root, &env);
        if failed.is_empty() {
            bail!("环境 {env} 没有最近失败的用例（运行历史 .codebaby/runs/ 中最近一次结论都是通过）");
        }
        let keep = crate::inspect::with_prerequisites(root, &failed);
        cases.retain(|c| keep.contains(&c.rel));
        if cases.is_empty() {
            bail!("最近失败的用例不在所选路径内：{}", failed.join(", "));
        }
    }
    // 单发：缺运行时变量（token / 前面用例捕获的 id）时先跑捕获它的用例，否则必然「占位符未替换」
    let mut prerequisites = vec![];
    let mut pre_notices = vec![];
    if let (true, false, [target], Some(root)) = (spec.prerequisites, spec.opts.dry_run, cases.as_slice(), &root) {
        let known = spec.seed.as_ref().map(|s| s.vars.keys().cloned().collect()).unwrap_or_default();
        let pre = crate::inspect::send_prerequisites(root, &target.path, &env, &known);
        if !pre.is_empty() {
            let all = discover::discover(&[root.join("collections")])?;
            let mut run_first: Vec<_> = pre.iter().filter_map(|(rel, _)| all.iter().find(|d| &d.rel == rel).cloned()).collect();
            for (rel, vars) in &pre {
                pre_notices.push(format!("先运行前置用例 {rel}：它捕获了本用例要用的 {}", vars.join("、")));
            }
            prerequisites = run_first.iter().map(|d| d.rel.clone()).collect();
            run_first.append(&mut cases);
            cases = run_first;
        }
    }
    // 对比基准：执行前读取（本次结果写入历史之前）
    let baseline = root.as_ref().filter(|_| !spec.opts.dry_run).map(|r| history::last_status(r, &env));
    let ctx = runner::load_project_ctx(&spec.paths[0], &env)?;
    let timeout_ms = ctx.project.timeout_ms;
    let emit = |ev: RunEvent| {
        if let Some(h) = &hook {
            h(&ev);
        }
    };
    let started_at_ms = history::now_ms();
    let id = history::new_run_id(&spec.source, started_at_ms);
    let mut opts = spec.opts.clone();
    if let (None, Some(root), false) = (&opts.artifacts, &root, opts.dry_run) {
        opts.artifacts = Some(history::artifacts_for(root, &id, spec.record_history));
    }

    on_start(&env, cases.len(), timeout_ms);
    emit(RunEvent::RunStarted { id: id.clone(), env: env.clone(), source: spec.source.clone(), planned: cases.len() });

    for n in &pre_notices {
        emit(RunEvent::Notice { message: n.clone() });
    }
    let mut r = Runner::new(ctx);
    if let Some(seed) = &spec.seed {
        r.seed(seed.clone());
    }
    if let Some(h) = &hook {
        r.set_hook(h.clone());
    }
    let results = r.run(&cases, &opts, |_| {})?;
    let mut notices = pre_notices;
    notices.extend(r.take_notices());
    let state = r.state();

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
        verify: false,
        git: root.as_deref().filter(|_| !spec.opts.dry_run).and_then(history::git_info),
    };
    let mut report = RunReport::new(&results, true, Some(meta));
    report.compare = baseline.and_then(|b| history::compare(&b, &report));
    if let (Some(c), Some(root)) = (report.compare.as_mut(), &root) {
        c.flaky = history::flaky(root, &env, &RunReport::new(&results, false, report.meta.clone()));
    }
    let history_path = match &root {
        Some(root) if spec.record_history && !spec.opts.dry_run => history::save(root, &report).ok(),
        _ => None,
    };
    emit(RunEvent::RunFinished { report: report.clone() });
    Ok(SessionOutcome { env, planned: cases.len(), timeout_ms, results, report, notices, history_path, state, prerequisites })
}

/// 项目根（路径向上查找 codebaby.json）
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
            "codebaby run collections/core -e dev --tag smoke,auth --name login --fail-fast --parallel 4 -o 'out dir'"
        );
        assert_eq!(RunRequest::default().command_line(), "codebaby run collections");
    }
}
