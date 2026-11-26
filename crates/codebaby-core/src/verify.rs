//! `codebaby verify` 先规划执行范围，再核对每个必验实例的断言证据。
//!
//! 阶段：校验 → 选中的流程，或 collections 与适用流程 → 逐项核对证据。
//! - 被测服务由使用者自行启动，codebaby 只发请求、给结论
//! - 流程固定了其他环境（如本地 fixture 只能跑 local）：本次跳过（未执行），在那个环境 verify 或单独运行该流程
//! - 用例失败不中止后续阶段：一次给出完整结论
//! - 内部阶段不单独写历史，整体只写一条（steps = 各阶段，issues = 校验问题）
//! - 事件流与流程同构（stepStarted / stepFinished = 阶段），CLI 与 Desktop 同一套渲染
use crate::config::{load_strict, Project, TestCase};
use crate::history;
use crate::report::{IssueReport, RunMeta, RunReport, StepReport, StepStatus, UnverifiedCase, UnverifiedReason, VerificationCoverage};
use crate::runner::{self, discover, CancelToken, CaseResult, EventHook, RunEvent, RunOptions};
use crate::session::{self, quote, RunRequest};
use crate::validate::{self, ValidationIssue};
use crate::workflow::{self, SessionBase, WorkflowSpec};
use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct VerifySpec {
    /// 项目根
    pub root: PathBuf,
    /// None 时取项目 defaultEnv
    pub env: Option<String>,
    pub selection: Selection,
    /// 响应落盘目录（相对项目根）
    pub output: Option<String>,
    /// 保留完整响应（Desktop 响应面板）
    pub send_mode: bool,
    pub source: String,
    pub record_history: bool,
    pub cancel: Option<CancelToken>,
}

#[derive(Debug, Default)]
pub struct Selection {
    pub workflow: Option<String>,
    /// 项目相对用例文件。为空时，当前计划中的所有用例实例均须验收。
    pub required_cases: Vec<String>,
}

/// 阶段（开始前已全部确定）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stage {
    #[serde(skip)]
    pub index: usize,
    #[serde(skip)]
    pub total: usize,
    pub label: String,
    pub kind: StageKind,
    /// 不在本次环境执行的原因（流程固定在其他环境）
    pub skip: Option<String>,
    /// 计划用例数（校验 / 跳过为 0）
    pub planned: usize,
    pub cases: Vec<PlannedCase>,
    #[serde(skip)]
    reuse_dependencies: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedCase {
    pub file: String,
    pub name: String,
    pub method: String,
    pub user: Option<String>,
    pub required: bool,
    pub skipped: bool,
    pub may_write: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retries: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StageKind {
    Validate,
    Cases,
    /// 流程 key
    Workflow(String),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanReport {
    pub env: String,
    pub stages: Vec<Stage>,
    pub missing_cases: Vec<String>,
    pub blocked: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<IssueReport>,
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
    command_line_with(env, output, &Selection::default())
}

fn command_line_with(env: Option<&str>, output: Option<&str>, selection: &Selection) -> String {
    let mut s = "codebaby verify".to_string();
    if let Some(e) = env {
        s.push_str(&format!(" -e {}", quote(e)));
    }
    if let Some(o) = output.filter(|o| !o.is_empty()) {
        s.push_str(&format!(" -o {}", quote(o)));
    }
    if let Some(workflow) = &selection.workflow {
        s.push_str(&format!(" --workflow {}", quote(workflow)));
    }
    for file in &selection.required_cases {
        s.push_str(&format!(" --require-case {}", quote(file)));
    }
    s
}

/// 规划阶段：环境名 + 全部阶段（计划用例数在开始前确定，Desktop stepper 一次展示完整）
pub fn plan(root: &Path, env: Option<&str>) -> Result<(Project, String, Vec<Stage>)> {
    plan_with(root, env, &Selection::default())
}

fn plan_with(root: &Path, env: Option<&str>, selection: &Selection) -> Result<(Project, String, Vec<Stage>)> {
    let pf = discover::project_file(root).ok_or_else(|| anyhow!("缺少 codebaby.json（{}）", root.display()))?;
    let project = load_strict::<Project>(&pf).context("项目配置错误")?;
    let workflow = selection.workflow.as_deref().map(|name| {
        project.workflows.get(name).ok_or_else(|| anyhow!("流程 {name} 不存在")).map(|wf| (name, wf))
    }).transpose()?;
    let env = match workflow {
        Some((name, wf)) => workflow::resolve_env(&project, name, wf, env)?,
        None => env.unwrap_or(&project.default_env).to_string(),
    };
    let required = required_paths(selection)?;
    let collections = root.join("collections");
    let mut kinds = vec![StageKind::Validate];
    if let Some((name, _)) = workflow {
        kinds.push(StageKind::Workflow(name.to_string()));
    } else {
        if collections.is_dir() {
            kinds.push(StageKind::Cases);
        }
        kinds.extend(project.workflows.keys().map(|k| StageKind::Workflow(k.clone())));
    }
    let total = kinds.len();
    // 依赖相同的流程（如都依赖「登录」）在 verify 里共用同一次登录：先用到的负责跑，后面的直接沿用
    let mut seen: Vec<BTreeSet<String>> = vec![];
    let stages = kinds
        .into_iter()
        .enumerate()
        .map(|(index, kind)| {
            let mut skip = None;
            let mut reuse_dependencies = false;
            let mut problems = vec![];
            let (label, cases) = match &kind {
                StageKind::Validate => ("校验".to_string(), vec![]),
                StageKind::Cases => ("全部用例".to_string(), planned_cases(&collections, &RunOptions::default(), None, &required)?),
                StageKind::Workflow(k) => {
                    let wf = &project.workflows[k];
                    let name = if wf.name.is_empty() { k.clone() } else { wf.name.clone() };
                    match wf.env.as_ref().filter(|e| **e != env) {
                        Some(own) => {
                            skip = Some(format!("固定在环境 {own}，本次跳过（codebaby workflow {k} 或 verify -e {own} 运行）"));
                            (format!("流程 {name}（{own}）"), vec![])
                        }
                        None => {
                            let closure: BTreeSet<String> = workflow::needs_closure(&project, k).unwrap_or_default().into_iter().collect();
                            let shared = !closure.is_empty() && seen.contains(&closure);
                            reuse_dependencies = shared;
                            let none = BTreeSet::new();
                            let steps = match workflow::expand(&project, k, if shared { &closure } else { &none }, false) {
                                Ok(steps) => steps,
                                Err(e) => {
                                    problems.push(e.to_string());
                                    vec![]
                                }
                            };
                            let mut cases = vec![];
                            for step in steps.into_iter().map(|p| p.step) {
                                let opts = RunOptions { tags: step.tags, name_filter: step.name_filter, ..Default::default() };
                                let selected = planned_cases(&root.join(&step.run), &opts, step.as_user.as_deref(), &required)?;
                                if selected.is_empty() {
                                    problems.push(format!("流程 {k} 的步骤 {} 没有匹配到用例", step.run));
                                }
                                cases.extend(selected);
                            }
                            if !closure.is_empty() && !shared {
                                seen.push(closure);
                            }
                            (format!("流程 {name}"), cases)
                        }
                    }
                }
            };
            Ok(Stage { index, total, label, kind, skip, planned: cases.len(), cases, reuse_dependencies, problems })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok((project, env, stages))
}

fn required_paths(selection: &Selection) -> Result<BTreeSet<String>> {
    selection.required_cases.iter().map(|file| {
        let normalized = file.replace('\\', "/");
        let mut parts = vec![];
        for part in Path::new(&normalized).components() {
            match part {
                std::path::Component::Normal(part) => parts.push(part.to_string_lossy()),
                std::path::Component::CurDir => {}
                _ => bail!("必验用例须使用项目相对路径：{file}"),
            }
        }
        let normalized = parts.join("/");
        if !normalized.ends_with(".json") {
            bail!("必验用例须指定 JSON 文件：{file}");
        }
        Ok(normalized)
    }).collect()
}

fn planned_cases(path: &Path, opts: &RunOptions, user: Option<&str>, required: &BTreeSet<String>) -> Result<Vec<PlannedCase>> {
    if !path.exists() {
        return Ok(vec![]); // 路径错误由后续 validate 报告
    }
    let mut cases = vec![];
    for dc in discover::discover(std::slice::from_ref(&path.to_path_buf()))? {
        let Ok(case) = load_strict::<TestCase>(&dc.path) else { continue }; // 保留 validate 的结构化错误
        if !runner::selected_case(&dc, &case, opts) {
            continue;
        }
        let protocol = case.protocol();
        let method = case.request.as_ref().map_or_else(|| format!("{protocol:?}"), |r| r.method.clone());
        let read_method = case.request.as_ref().is_some_and(|r| ["GET", "HEAD", "OPTIONS"].iter().any(|m| r.method.eq_ignore_ascii_case(m)));
        let may_write = !read_method
            || !case.pre.is_empty()
            || !case.post.is_empty()
            || case.tests.iter().any(|test| test.contains("http.") || test.contains("__httpRequest"))
            || dc.chain.iter().any(|c| !c.setup.is_empty() || !c.teardown.is_empty());
        let retries = case.request.as_ref().and_then(|r| r.retry).filter(|n| *n > 0);
        let name = runner::display_name(&dc, &case);
        let rows = runner::case_rows(&dc, &case).unwrap_or_else(|_| vec![None]);
        for (index, row) in rows.iter().enumerate() {
            cases.push(PlannedCase {
                file: dc.rel.clone(),
                name: runner::instance_name(&name, index, row.as_ref()),
                method: method.clone(),
                user: user.map(str::to_string),
                required: required.is_empty() || required.contains(&dc.rel),
                skipped: case.skip,
                may_write,
                retries,
            });
        }
    }
    Ok(cases)
}

fn missing_cases(stages: &[Stage], selection: &Selection) -> Result<Vec<String>> {
    let selected: BTreeSet<_> = stages.iter().flat_map(|s| &s.cases).map(|c| c.file.as_str()).collect();
    Ok(required_paths(selection)?.into_iter().filter(|path| !selected.contains(path.as_str())).collect())
}

fn plan_blocker(stages: &[Stage], missing: &[String]) -> Option<String> {
    if !missing.is_empty() {
        return Some(format!("{} 个必验用例未进入执行计划：{}", missing.len(), missing.join("、")));
    }
    let problems: Vec<_> = stages.iter().flat_map(|s| &s.problems).map(String::as_str).collect();
    if !problems.is_empty() {
        return Some(problems.join("；"));
    }
    if !stages.iter().flat_map(|s| &s.cases).any(|c| c.required) {
        return Some("本次计划没有必验用例".into());
    }
    // 独立扫描和流程的叠加是隐式重复。两个命名流程可有意重复访问同一用例。
    let standalone: BTreeSet<_> = stages.iter().filter(|s| s.kind == StageKind::Cases)
        .flat_map(|s| &s.cases).filter(|c| c.may_write && !c.skipped).map(|c| c.file.as_str()).collect();
    let mut duplicates = BTreeSet::new();
    for stage in stages.iter().filter(|s| matches!(s.kind, StageKind::Workflow(_))) {
        for case in stage.cases.iter().filter(|c| c.may_write && !c.skipped) {
            if standalone.contains(case.file.as_str()) {
                duplicates.insert(case.file.as_str());
            }
        }
    }
    (!duplicates.is_empty()).then(|| format!(
        "可能有副作用的用例进入了多个阶段：{}。使用 --workflow 选择一个流程，或将独立集合和流程用例分开",
        duplicates.into_iter().collect::<Vec<_>>().join("、")
    ))
}

pub fn preview(root: &Path, env: Option<&str>, selection: &Selection) -> Result<PlanReport> {
    let (project, env, stages) = plan_with(root, env, selection)?;
    let missing_cases = missing_cases(&stages, selection)?;
    let issues = validate_all(root, &env, &project, selection.workflow.as_deref())?;
    let blocked = validation_blocker(&issues, &env).or_else(|| plan_blocker(&stages, &missing_cases));
    Ok(PlanReport { env, stages, missing_cases, blocked, issues: issues.iter().map(IssueReport::from).collect() })
}

pub fn run(spec: &VerifySpec, hook: Option<EventHook>, mut on_stage: impl FnMut(StagePhase<'_>)) -> Result<VerifyOutcome> {
    let (project, env, stages) = plan_with(&spec.root, spec.env.as_deref(), &spec.selection)?;
    let missing = missing_cases(&stages, &spec.selection)?;
    let plan_problem = plan_blocker(&stages, &missing);
    let mut coverage = VerificationCoverage {
        required: missing.len() + stages.iter().flat_map(|s| &s.cases).filter(|c| c.required).count(),
        checked: 0,
        unverified: missing.into_iter().map(|file| UnverifiedCase {
            name: file.clone(), file, stage: String::new(), user: None, reason: UnverifiedReason::NotSelected,
        }).collect(),
    };
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
    // 流程依赖的登录态：依赖相同的流程共用，第一个用到的负责跑
    let mut bases: Vec<SessionBase> = vec![];

    for stage in &stages {
        if blocked.is_some() || cancelled() {
            steps.push(step_report(stage, &[], StepStatus::NotRun));
            check_coverage(&mut coverage, stage, &[]);
            continue;
        }
        on_stage(StagePhase::Started(stage));
        let base = match &stage.kind {
            StageKind::Workflow(key) if stage.reuse_dependencies => workflow::needs_closure(&project, key)
                .ok().map(|c| c.into_iter().collect::<BTreeSet<_>>())
                .and_then(|c| bases.iter().find(|b| b.satisfied == c).cloned()),
            _ => None,
        };
        let skip = stage.skip.as_deref().or_else(|| {
            (stage.reuse_dependencies && base.is_none()).then_some("前序流程未建立可共享的依赖状态")
        });
        if let Some(why) = skip {
            on_stage(StagePhase::Finished(stage, &StageOutcome::Skipped(why)));
            steps.push(step_report(stage, &[], StepStatus::NotRun));
            check_coverage(&mut coverage, stage, &[]);
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
                match validate_all(&spec.root, &env, &project, spec.selection.workflow.as_deref()) {
                    Ok(found) => {
                        issues = found;
                        on_stage(StagePhase::Finished(stage, &StageOutcome::Validated(&issues)));
                        blocked = validation_blocker(&issues, &env).or_else(|| plan_problem.clone());
                        if blocked.is_some() { StepStatus::Failed } else { StepStatus::Passed }
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
                    skip_needs: false,
                    base,
                };
                match workflow::run(&ws, inner_hook.clone(), |_| {}) {
                    Ok(o) => {
                        if !o.base.satisfied.is_empty() && !bases.iter().any(|b| b.satisfied == o.base.satisfied) {
                            bases.push(o.base);
                        }
                        notices.extend(o.notices);
                        ran = o.results;
                        finished_ran(stage, &ran, &mut on_stage)
                    }
                    Err(e) => block(&mut blocked, stage, &mut on_stage, format!("{e:#}")),
                }
            }
        };
        let before = coverage.unverified.len();
        check_coverage(&mut coverage, stage, &ran);
        let status = if coverage.unverified.len() > before { StepStatus::Failed } else { status };
        let report = step_report(stage, &ran, status);
        emit(RunEvent::StepFinished { index: stage.index, passed: report.passed, failed: report.failed, skipped: report.skipped });
        steps.push(report);
        results.extend(ran);
    }

    let meta = RunMeta {
        id,
        source: spec.source.clone(),
        command: command_line_with(Some(&env), spec.output.as_deref(), &spec.selection),
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
    if blocked.is_none() {
        if cancelled() {
            blocked = Some("验证已取消".into());
        } else if report.failed == 0 && !coverage.unverified.is_empty() {
            blocked = Some(coverage.summary());
        }
    }
    report.steps = steps;
    report.issues = issues.iter().map(IssueReport::from).collect();
    report.blocked = blocked.clone();
    report.coverage = Some(coverage);
    report.compare = history::compare(&baseline, &report);
    if let Some(c) = report.compare.as_mut() {
        c.flaky = history::flaky(&spec.root, &env, &RunReport::new(&results, false, report.meta.clone()));
    }
    let history_path = if spec.record_history { history::save(&spec.root, &report).ok() } else { None };
    emit(RunEvent::RunFinished { report: report.clone() });
    Ok(VerifyOutcome { env, stages, issues, results, report, notices, blocked, history_path })
}

/// 校验：范围见 validate::project_scope（固定在其他环境的流程本次不校验）
fn validate_all(root: &Path, env: &str, project: &Project, selected: Option<&str>) -> Result<Vec<ValidationIssue>> {
    let Some(key) = selected else { return validate::validate_paths(&validate::project_scope(root, env), env) };
    let mut keys: BTreeSet<_> = workflow::needs_closure(project, key)?.into_iter().collect();
    keys.insert(key.to_string());
    let mut paths = BTreeSet::from_iter(discover::project_file(root));
    for key in &keys {
        paths.extend(project.workflows[key].steps.iter().map(|step| root.join(&step.run)).filter(|p| p.exists()));
    }
    validate::validate_selected(&paths.into_iter().collect::<Vec<_>>(), env, Some(&keys))
}

fn validation_blocker(issues: &[ValidationIssue], env: &str) -> Option<String> {
    let errors = issues.iter().filter(|i| i.level == "error").count();
    (errors > 0).then(|| format!("校验发现 {errors} 个错误：修复后再验证（环境 {env}）"))
}

fn check_coverage(coverage: &mut VerificationCoverage, stage: &Stage, results: &[CaseResult]) {
    let mut observed = BTreeMap::<_, VecDeque<_>>::new();
    for r in results {
        observed.entry((r.rel.as_str(), r.name.as_str())).or_default().push_back(r);
    }
    for case in stage.cases.iter().filter(|c| c.required) {
        let result = observed.get_mut(&(case.file.as_str(), case.name.as_str())).and_then(VecDeque::pop_front);
        let reason = match result {
            None => UnverifiedReason::NotRun,
            Some(r) if r.skipped => UnverifiedReason::Skipped,
            Some(r) if r.error.is_some() && r.tests.is_empty() => UnverifiedReason::ExecutionError,
            Some(r) if r.tests.is_empty() => UnverifiedReason::NoAssertion,
            Some(_) => { coverage.checked += 1; continue; }
        };
        coverage.unverified.push(UnverifiedCase {
            file: case.file.clone(), name: case.name.clone(), stage: stage.label.clone(), user: case.user.clone(), reason,
        });
    }
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

