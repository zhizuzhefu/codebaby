//! 工作流编排：`requestbaby.json` 中命名的多阶段运行。
//!
//! 各步骤共享同一个 Runner（运行时变量跨步骤自动传递），步骤内 fail-fast，
//! 任一步骤失败即中止后续步骤（dry-run 除外）。CLI `workflow` 与 Desktop 共用本模块。
use crate::config::{load_strict, Project, Workflow, WorkflowStep};
use crate::history;
use crate::report::{RunMeta, RunReport, StepReport, StepStatus};
use crate::runner::{self, discover, CancelToken, CaseResult, EventHook, RunEvent, RunOptions, Runner};
use crate::session::quote;
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};

pub struct WorkflowSpec {
    /// 项目根
    pub root: PathBuf,
    /// 工作流 key
    pub name: String,
    /// None 时取项目 defaultEnv
    pub env: Option<String>,
    pub dry_run: bool,
    pub verbose: bool,
    /// 保留完整响应（Desktop 响应面板）
    pub send_mode: bool,
    pub source: String,
    pub record_history: bool,
    pub cancel: Option<CancelToken>,
}

/// 步骤信息（开始前已知）
#[derive(Debug, Clone)]
pub struct StepInfo {
    pub index: usize,
    pub total: usize,
    /// 展示名：name 缺省为 run 路径
    pub label: String,
    pub run: String,
    pub tags: Vec<String>,
    pub name_filter: String,
    /// 该步骤发现的用例数（过滤前）
    pub planned: usize,
}

pub enum StepPhase<'a> {
    Started(&'a StepInfo),
    Finished(&'a StepInfo, &'a [CaseResult]),
}

pub struct WorkflowOutcome {
    pub env: String,
    pub display_name: String,
    pub steps: Vec<StepInfo>,
    /// 失败中止的步骤（0 起）
    pub failed_step: Option<usize>,
    pub results: Vec<CaseResult>,
    pub report: RunReport,
    pub notices: Vec<String>,
    pub history_path: Option<PathBuf>,
}

/// 读取项目配置中的工作流；不存在时列出已定义名称
pub fn load(root: &Path, name: &str) -> Result<(Project, Workflow)> {
    let pf = discover::project_file(root).ok_or_else(|| anyhow!("缺少 requestbaby.json（{}）", root.display()))?;
    let project = load_strict::<Project>(&pf).context("项目配置错误")?;
    let wf = project.workflows.get(name).cloned().ok_or_else(|| {
        let defined: Vec<String> = project.workflows.keys().cloned().collect();
        anyhow!(
            "工作流 {name} 不存在。已定义: {}",
            if defined.is_empty() { "（无）".to_string() } else { defined.join(", ") }
        )
    })?;
    Ok((project, wf))
}

/// 等价 CLI 命令
pub fn command_line(name: &str, env: Option<&str>, dry_run: bool) -> String {
    let mut s = format!("requestbaby workflow {}", quote(name));
    if let Some(e) = env {
        s.push_str(&format!(" -e {}", quote(e)));
    }
    if dry_run {
        s.push_str(" --dry-run");
    }
    s
}

pub fn step_label(step: &WorkflowStep) -> String {
    if step.name.is_empty() {
        step.run.clone()
    } else {
        step.name.clone()
    }
}

pub fn run(spec: &WorkflowSpec, hook: Option<EventHook>, mut on_step: impl FnMut(StepPhase<'_>)) -> Result<WorkflowOutcome> {
    let (project, wf) = load(&spec.root, &spec.name)?;
    let env = spec.env.clone().unwrap_or_else(|| project.default_env.clone());
    let ctx = runner::load_project_ctx(&spec.root, &env)?;
    let display_name = if wf.name.is_empty() { spec.name.clone() } else { wf.name.clone() };

    // 先发现全部步骤：配置错误在执行任何请求前暴露
    let total = wf.steps.len();
    let mut planned = vec![];
    for (index, step) in wf.steps.iter().enumerate() {
        let target = spec.root.join(&step.run);
        if !target.exists() {
            return Err(anyhow!("步骤 {} 的路径不存在: {}", index + 1, step.run));
        }
        let cases = discover::discover(std::slice::from_ref(&target))
            .with_context(|| format!("步骤 {} 发现用例失败", step.run))?;
        let info = StepInfo {
            index,
            total,
            label: step_label(step),
            run: step.run.clone(),
            tags: step.tags.clone(),
            name_filter: step.name_filter.clone(),
            planned: cases.len(),
        };
        planned.push((info, cases));
    }

    let started_at_ms = history::now_ms();
    let id = history::new_run_id(&spec.source, started_at_ms);
    let emit = |ev: RunEvent| {
        if let Some(h) = &hook {
            h(&ev);
        }
    };
    emit(RunEvent::RunStarted {
        id: id.clone(),
        env: env.clone(),
        source: spec.source.clone(),
        planned: planned.iter().map(|(i, _)| i.planned).sum(),
    });

    let mut runner = Runner::new(ctx);
    if let Some(h) = &hook {
        runner.set_hook(h.clone());
    }
    let cancelled = || spec.cancel.as_ref().is_some_and(|c| c.is_cancelled());
    let mut all: Vec<CaseResult> = vec![];
    let mut step_reports: Vec<StepReport> = vec![];
    let mut failed_step = None;

    for (info, cases) in &planned {
        if failed_step.is_some() || cancelled() {
            step_reports.push(step_report(info, &[], StepStatus::NotRun));
            continue;
        }
        on_step(StepPhase::Started(info));
        emit(RunEvent::StepStarted {
            index: info.index,
            total,
            label: info.label.clone(),
            run: info.run.clone(),
            planned: info.planned,
        });
        let opts = RunOptions {
            tags: info.tags.clone(),
            name_filter: info.name_filter.clone(),
            fail_fast: true,
            dry_run: spec.dry_run,
            verbose: spec.verbose,
            send_mode: spec.send_mode,
            cancel: spec.cancel.clone(),
            ..Default::default()
        };
        let results = runner.run(cases, &opts, |_| {}).with_context(|| format!("步骤 {} 执行失败", info.label))?;
        let failed = results.iter().any(|r| !r.passed() && !r.skipped);
        let report = step_report(info, &results, if failed { StepStatus::Failed } else { StepStatus::Passed });
        emit(RunEvent::StepFinished {
            index: info.index,
            passed: report.passed,
            failed: report.failed,
            skipped: report.skipped,
        });
        on_step(StepPhase::Finished(info, &results));
        step_reports.push(report);
        all.extend(results);
        if failed && !spec.dry_run {
            failed_step = Some(info.index);
        }
    }

    let notices = runner.take_notices();
    let meta = RunMeta {
        id,
        source: spec.source.clone(),
        command: command_line(&spec.name, Some(&env), spec.dry_run),
        env: env.clone(),
        started_at_ms,
        finished_at_ms: history::now_ms(),
        dry_run: spec.dry_run,
        cancelled: cancelled(),
        workflow: Some(spec.name.clone()),
    };
    let mut report = RunReport::new(&all, true, Some(meta));
    report.steps = step_reports;
    let history_path = if spec.record_history && !spec.dry_run { history::save(&spec.root, &report).ok() } else { None };
    emit(RunEvent::RunFinished { report: report.clone() });
    Ok(WorkflowOutcome {
        env,
        display_name,
        steps: planned.into_iter().map(|(i, _)| i).collect(),
        failed_step,
        results: all,
        report,
        notices,
        history_path,
    })
}

fn step_report(info: &StepInfo, results: &[CaseResult], status: StepStatus) -> StepReport {
    StepReport {
        label: info.label.clone(),
        run: info.run.clone(),
        tags: info.tags.clone(),
        name_filter: info.name_filter.clone(),
        status,
        total: results.len(),
        passed: results.iter().filter(|r| r.passed()).count(),
        failed: results.iter().filter(|r| !r.passed() && !r.skipped).count(),
        skipped: results.iter().filter(|r| r.skipped).count(),
    }
}
