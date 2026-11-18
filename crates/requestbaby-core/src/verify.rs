//! 完整验证 `requestbaby verify`：一条命令给出「当前代码是否满足全部用例」的结论。
//!
//! 阶段：校验（有 error 即中止）→ 全部用例（collections，文件序）→ 每个流程（按 key 序）。
//! - 被测服务由使用者自行启动，requestbaby 只发请求、给结论
//! - 流程固定了其他环境（如本地 fixture 只能跑 local）：本次跳过（未执行），在那个环境 verify 或单独运行该流程
//! - 用例失败不中止后续阶段：一次给出完整结论
//! - 内部阶段不单独写历史，整体只写一条（steps = 各阶段，issues = 校验问题）
//! - 事件流与流程同构（stepStarted / stepFinished = 阶段），CLI 与 Desktop 同一套渲染
use crate::config::{load_strict, Project};
use crate::history;
use crate::report::{IssueReport, RunMeta, RunReport, StepReport, StepStatus};
use crate::runner::{discover, CancelToken, CaseResult, EventHook, RunEvent};
use crate::session::{self, quote, RunRequest};
use crate::validate::{self, ValidationIssue};
use crate::workflow::{self, WorkflowSpec};
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct VerifySpec {
    /// 项目根
    pub root: PathBuf,
    /// None 时取项目 defaultEnv
    pub env: Option<String>,
    /// 响应落盘目录（相对项目根）
    pub output: Option<String>,
    /// 保留完整响应（Desktop 响应面板）
    pub send_mode: bool,
    pub source: String,
    pub record_history: bool,
    pub cancel: Option<CancelToken>,
}

/// 阶段（开始前已全部确定）
#[derive(Debug, Clone)]
pub struct Stage {
    pub index: usize,
    pub total: usize,
    pub label: String,
    pub kind: StageKind,
    /// 不在本次环境执行的原因（流程固定在其他环境）
    pub skip: Option<String>,
    /// 计划用例数（校验 / 跳过为 0）
    pub planned: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageKind {
    Validate,
    Cases,
    /// 流程 key
    Workflow(String),
}

impl StageKind {
    /// 报告中的 run 字段：可直接复现的目标
    fn run(&self) -> String {
        match self {
            StageKind::Validate => "validate".into(),
            StageKind::Cases => "collections".into(),
            StageKind::Workflow(k) => format!("workflow {k}"),
        }
    }
}

pub enum StagePhase<'a> {
    Started(&'a Stage),
    /// 阶段结束：执行类阶段附带用例结果
    Finished(&'a Stage, &'a StageOutcome<'a>),
}

pub enum StageOutcome<'a> {
    Validated(&'a [ValidationIssue]),
    /// 阶段未执行：原因
    Skipped(&'a str),
    Ran(&'a [CaseResult]),
    /// 阶段无法执行（配置错误等）
    Error(&'a str),
}

pub struct VerifyOutcome {
    pub env: String,
    pub stages: Vec<Stage>,
    pub issues: Vec<ValidationIssue>,
    pub results: Vec<CaseResult>,
    pub report: RunReport,
    pub notices: Vec<String>,
    /// 阻断性问题（校验 error / 阶段配置错误）：退出码 2
    pub blocked: Option<String>,
    pub history_path: Option<PathBuf>,
}

impl VerifyOutcome {
    /// 0 全部通过 · 1 有用例失败 · 2 配置 / 环境问题导致无法给出结论
    pub fn exit_code(&self) -> u8 {
        if self.blocked.is_some() {
            2
        } else if self.report.failed > 0 {
            1
        } else {
            0
        }
    }
}

/// 等价 CLI 命令
pub fn command_line(env: Option<&str>, output: Option<&str>) -> String {
    let mut s = "requestbaby verify".to_string();
    if let Some(e) = env {
        s.push_str(&format!(" -e {}", quote(e)));
    }
    if let Some(o) = output.filter(|o| !o.is_empty()) {
        s.push_str(&format!(" -o {}", quote(o)));
    }
    s
}

/// 规划阶段：环境名 + 全部阶段（计划用例数在开始前确定，Desktop stepper 一次展示完整）
pub fn plan(root: &Path, env: Option<&str>) -> Result<(Project, String, Vec<Stage>)> {
    let pf = discover::project_file(root).ok_or_else(|| anyhow!("缺少 requestbaby.json（{}）", root.display()))?;
    let project = load_strict::<Project>(&pf).context("项目配置错误")?;
    let env = env.map(String::from).unwrap_or_else(|| project.default_env.clone());
    let collections = root.join("collections");
    let mut kinds = vec![StageKind::Validate];
    if collections.is_dir() {
        kinds.push(StageKind::Cases);
    }
    kinds.extend(project.workflows.keys().map(|k| StageKind::Workflow(k.clone())));
    let total = kinds.len();
    let stages = kinds
        .into_iter()
        .enumerate()
        .map(|(index, kind)| {
            let mut skip = None;
            let (label, planned) = match &kind {
                StageKind::Validate => ("校验".to_string(), 0),
                StageKind::Cases => ("全部用例".to_string(), count(std::slice::from_ref(&collections))),
                StageKind::Workflow(k) => {
                    let wf = &project.workflows[k];
                    let name = if wf.name.is_empty() { k.clone() } else { wf.name.clone() };
                    let targets: Vec<PathBuf> = wf.steps.iter().map(|s| root.join(&s.run)).collect();
                    match wf.env.as_ref().filter(|e| **e != env) {
                        Some(own) => {
                            skip = Some(format!("固定在环境 {own}，本次跳过（requestbaby workflow {k} 或 verify -e {own} 运行）"));
                            (format!("流程 {name}（{own}）"), 0)
                        }
                        None => (format!("流程 {name}"), count(&targets)),
                    }
                }
            };
            Stage { index, total, label, kind, skip, planned }
        })
        .collect();
    Ok((project, env, stages))
}

pub fn run(spec: &VerifySpec, hook: Option<EventHook>, mut on_stage: impl FnMut(StagePhase<'_>)) -> Result<VerifyOutcome> {
    let (_, env, stages) = plan(&spec.root, spec.env.as_deref())?;
    let total = stages.len();

    let emit = |ev: RunEvent| {
        if let Some(h) = &hook {
            h(&ev);
        }
    };
    // 内部运行的事件：用例 / 日志 / 提示透传；其运行级与步骤级事件由本层的阶段事件代替
    let inner_hook: Option<EventHook> = hook.clone().map(|h| {
        Arc::new(move |ev: &RunEvent| {
            if !matches!(
                ev,
                RunEvent::RunStarted { .. } | RunEvent::RunFinished { .. } | RunEvent::StepStarted { .. } | RunEvent::StepFinished { .. }
            ) {
                h(ev)
            }
        }) as EventHook
    });

    let started_at_ms = history::now_ms();
    // 对比基准：执行前读取
    let baseline = history::last_status(&spec.root, &env);
    let id = history::new_run_id(&spec.source, started_at_ms);
    emit(RunEvent::RunStarted {
        id: id.clone(),
        env: env.clone(),
        source: spec.source.clone(),
        planned: stages.iter().map(|s| s.planned).sum(),
    });

    let cancelled = || spec.cancel.as_ref().is_some_and(|c| c.is_cancelled());
    let mut issues: Vec<ValidationIssue> = vec![];
    let mut results: Vec<CaseResult> = vec![];
    let mut notices: Vec<String> = vec![];
    let mut steps: Vec<StepReport> = vec![];
    let mut blocked: Option<String> = None;

    for stage in &stages {
        if blocked.is_some() || cancelled() {
            steps.push(step_report(stage, &[], StepStatus::NotRun));
            continue;
        }
        on_stage(StagePhase::Started(stage));
        if let Some(why) = &stage.skip {
            on_stage(StagePhase::Finished(stage, &StageOutcome::Skipped(why)));
            steps.push(step_report(stage, &[], StepStatus::NotRun));
            continue;
        }
        emit(RunEvent::StepStarted {
            index: stage.index,
            total,
            label: stage.label.clone(),
            run: stage.kind.run(),
            planned: stage.planned,
        });
        let mut ran: Vec<CaseResult> = vec![];
        let status = match &stage.kind {
            StageKind::Validate => {
                match validate_all(&spec.root, &env) {
                    Ok(found) => {
                        issues = found;
                        let errors = issues.iter().filter(|i| i.level == "error").count();
                        on_stage(StagePhase::Finished(stage, &StageOutcome::Validated(&issues)));
                        if errors > 0 {
                            blocked = Some(format!("校验发现 {errors} 个错误：修复后再验证（requestbaby validate -e {env}）"));
                            StepStatus::Failed
                        } else {
                            StepStatus::Passed
                        }
                    }
                    Err(e) => block(&mut blocked, stage, &mut on_stage, format!("{e:#}")),
                }
            }
            StageKind::Cases => {
                let req = RunRequest { env: Some(env.clone()), output: spec.output.clone(), ..Default::default() };
                let mut s = req.to_spec(&spec.root, &spec.source, spec.send_mode, spec.cancel.clone());
                s.record_history = false;
                s.opts.artifacts = Some(history::artifacts_for(&spec.root, &id, spec.record_history));
                match session::run(&s, inner_hook.clone(), |_, _, _| {}) {
                    Ok(o) => {
                        notices.extend(o.notices);
                        ran = o.results;
                        finished_ran(stage, &ran, &mut on_stage)
                    }
                    Err(e) => block(&mut blocked, stage, &mut on_stage, format!("{e:#}")),
                }
            }
            StageKind::Workflow(key) => {
                let ws = WorkflowSpec {
                    root: spec.root.clone(),
                    name: key.clone(),
                    env: Some(env.clone()),
                    dry_run: false,
                    verbose: false,
                    send_mode: spec.send_mode,
                    source: spec.source.clone(),
                    record_history: false,
                    cancel: spec.cancel.clone(),
                    artifacts: Some(history::artifacts_for(&spec.root, &id, spec.record_history)),
                };
                match workflow::run(&ws, inner_hook.clone(), |_| {}) {
                    Ok(o) => {
                        notices.extend(o.notices);
                        ran = o.results;
                        finished_ran(stage, &ran, &mut on_stage)
                    }
                    Err(e) => block(&mut blocked, stage, &mut on_stage, format!("{e:#}")),
                }
            }
        };
        let report = step_report(stage, &ran, status);
        emit(RunEvent::StepFinished { index: stage.index, passed: report.passed, failed: report.failed, skipped: report.skipped });
        steps.push(report);
        results.extend(ran);
    }

    let meta = RunMeta {
        id,
        source: spec.source.clone(),
        command: command_line(Some(&env), spec.output.as_deref()),
        env: env.clone(),
        started_at_ms,
        finished_at_ms: history::now_ms(),
        dry_run: false,
        cancelled: cancelled(),
        workflow: None,
        verify: true,
        git: history::git_info(&spec.root),
    };
    let mut report = RunReport::new(&results, true, Some(meta));
    report.steps = steps;
    report.issues = issues.iter().map(IssueReport::from).collect();
    report.blocked = blocked.clone();
    report.compare = history::compare(&baseline, &report);
    if let Some(c) = report.compare.as_mut() {
        c.flaky = history::flaky(&spec.root, &env, &RunReport::new(&results, false, report.meta.clone()));
    }
    let history_path = if spec.record_history { history::save(&spec.root, &report).ok() } else { None };
    emit(RunEvent::RunFinished { report: report.clone() });
    Ok(VerifyOutcome { env, stages, issues, results, report, notices, blocked, history_path })
}

/// 校验：范围见 validate::project_scope（固定在其他环境的流程本次不校验）
fn validate_all(root: &Path, env: &str) -> Result<Vec<ValidationIssue>> {
    validate::validate_paths(&validate::project_scope(root, env), env)
}

fn count(targets: &[PathBuf]) -> usize {
    targets.iter().filter(|t| t.exists()).filter_map(|t| discover::discover(std::slice::from_ref(t)).ok()).map(|c| c.len()).sum()
}

fn finished_ran(stage: &Stage, ran: &[CaseResult], on_stage: &mut impl FnMut(StagePhase<'_>)) -> StepStatus {
    on_stage(StagePhase::Finished(stage, &StageOutcome::Ran(ran)));
    if ran.iter().any(|r| !r.passed() && !r.skipped) {
        StepStatus::Failed
    } else {
        StepStatus::Passed
    }
}

fn block(blocked: &mut Option<String>, stage: &Stage, on_stage: &mut impl FnMut(StagePhase<'_>), why: String) -> StepStatus {
    on_stage(StagePhase::Finished(stage, &StageOutcome::Error(&why)));
    *blocked = Some(if why.starts_with(&stage.label) { why } else { format!("{}：{why}", stage.label) });
    StepStatus::Failed
}

fn step_report(stage: &Stage, results: &[CaseResult], status: StepStatus) -> StepReport {
    StepReport {
        label: stage.label.clone(),
        run: stage.kind.run(),
        tags: vec![],
        name_filter: String::new(),
        status,
        total: results.len(),
        passed: results.iter().filter(|r| r.passed()).count(),
        failed: results.iter().filter(|r| !r.passed() && !r.skipped).count(),
        skipped: results.iter().filter(|r| r.skipped).count(),
    }
}

