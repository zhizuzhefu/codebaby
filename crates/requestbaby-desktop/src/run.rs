//! 运行控制：在后台线程执行 core 的 `session::run` / `workflow::run`，
//! 通过与 CLI `--json-stream` 相同的 [`RunEvent`] 协议把进度流回界面。
use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::{Context, EventEmitter, Task};
use requestbaby_core::report::{CaseReport, RunReport, StepStatus};
use requestbaby_core::runner::{CancelToken, CaseResult, EventHook, RunEvent};
use requestbaby_core::session::{self, RunRequest};
use requestbaby_core::workflow::{self, WorkflowSpec};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// 一次执行任务：与 CLI 的 `run` / `send` / `workflow` 一一对应
#[derive(Debug, Clone)]
pub enum RunJob {
    /// 运行集合 / 目录 / 用例（paths 相对项目根）：写入共享历史
    Run(RunRequest),
    /// 单用例调试发送：不写历史（与 CLI `send` 一致）
    Send { path: PathBuf, rel: String, env: String },
    /// 命名工作流：写入共享历史
    Workflow { name: String, env: String, dry_run: bool },
}

impl RunJob {
    pub fn command_line(&self) -> String {
        match self {
            RunJob::Run(r) => r.command_line(),
            RunJob::Send { path, rel, env } => session::send_spec(path.clone(), rel, Some(env.clone()), "desktop", None).command,
            RunJob::Workflow { name, env, dry_run } => workflow::command_line(name, Some(env), *dry_run),
        }
    }

    fn is_send(&self) -> bool {
        matches!(self, RunJob::Send { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Running,
    Finished,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct LogLine {
    pub file: String,
    pub text: String,
    pub notice: bool,
}

/// 工作流步骤视图（运行中实时更新；历史运行从报告还原）
#[derive(Debug, Clone)]
pub struct StepView {
    pub label: String,
    pub run: String,
    pub planned: usize,
    pub state: StepState,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    Pending,
    Running,
    Passed,
    Failed,
    NotRun,
}

/// 结果表的来源：本次运行，或历史中的某次运行
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResultsSource {
    Live,
    History(String),
}

pub enum RunStateEvent {
    /// 一次运行结束（含取消）；send 不写历史
    Finished { send: bool, workflow: Option<String>, failed: usize, total: usize, dry_run: bool },
    /// 单用例完成（结果回填响应面板）
    CaseDone { file: String },
    Error(String),
}

pub struct RunState {
    phase: Phase,
    job: Option<RunJob>,
    planned: usize,
    started_at: Option<Instant>,
    elapsed_ms: u64,
    current: Option<String>,
    rows: Vec<CaseReport>,
    steps: Vec<StepView>,
    workflow: Option<String>,
    source: ResultsSource,
    /// 结果表当前展示的报告（导出用）
    report: Option<RunReport>,
    /// 完整结果（含完整响应体），按 file 存最近一次实例与完成时间
    responses: HashMap<String, (u64, CaseResult)>,
    logs: Vec<LogLine>,
    command: String,
    cancel: Option<CancelToken>,
    _task: Option<Task<()>>,
}

enum Msg {
    Event(RunEvent),
    Done(anyhow::Result<(Vec<CaseResult>, RunReport)>),
}

impl EventEmitter<RunStateEvent> for RunState {}

impl RunState {
    pub fn new() -> Self {
        Self {
            phase: Phase::Idle,
            job: None,
            planned: 0,
            started_at: None,
            elapsed_ms: 0,
            current: None,
            rows: vec![],
            steps: vec![],
            workflow: None,
            source: ResultsSource::Live,
            report: None,
            responses: HashMap::new(),
            logs: vec![],
            command: String::new(),
            cancel: None,
            _task: None,
        }
    }

    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    pub fn is_running(&self) -> bool {
        self.phase == Phase::Running
    }

    /// 结果表正在实时更新（排除单发）
    pub fn is_streaming(&self) -> bool {
        self.is_running() && !self.job.as_ref().is_some_and(RunJob::is_send)
    }

    pub fn planned(&self) -> usize {
        self.planned
    }

    pub fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }

    pub fn rows(&self) -> &[CaseReport] {
        &self.rows
    }

    pub fn steps(&self) -> &[StepView] {
        &self.steps
    }

    pub fn workflow(&self) -> Option<&str> {
        self.workflow.as_deref()
    }

    pub fn source(&self) -> &ResultsSource {
        &self.source
    }

    pub fn report(&self) -> Option<&RunReport> {
        self.report.as_ref()
    }

    pub fn logs(&self) -> &[LogLine] {
        &self.logs
    }

    pub fn command(&self) -> &str {
        &self.command
    }

    pub fn elapsed_ms(&self) -> u64 {
        match (&self.phase, self.started_at) {
            (Phase::Running, Some(t)) if self.is_streaming() => t.elapsed().as_millis() as u64,
            _ => self.elapsed_ms,
        }
    }

    pub fn response(&self, file: &str) -> Option<&CaseResult> {
        self.responses.get(file).map(|(_, r)| r)
    }

    /// 本次会话结果的完成时间（与共享历史比较新旧）
    pub fn response_at(&self, file: &str) -> Option<u64> {
        self.responses.get(file).map(|(t, _)| *t)
    }

    /// 结果表展示历史中的一次运行（只读）
    pub fn show_report(&mut self, report: &RunReport, cx: &mut Context<Self>) {
        if self.is_streaming() {
            return;
        }
        let meta = report.meta.clone().unwrap_or_default();
        self.rows = report.results.clone();
        self.steps = report
            .steps
            .iter()
            .map(|s| StepView {
                label: s.label.clone(),
                run: s.run.clone(),
                planned: s.total,
                state: match s.status {
                    StepStatus::Passed => StepState::Passed,
                    StepStatus::Failed => StepState::Failed,
                    StepStatus::NotRun => StepState::NotRun,
                },
                passed: s.passed,
                failed: s.failed,
                skipped: s.skipped,
            })
            .collect();
        self.workflow = meta.workflow.clone();
        self.source = ResultsSource::History(meta.id.clone());
        self.command = meta.command.clone();
        self.planned = report.total;
        self.elapsed_ms = meta.finished_at_ms.saturating_sub(meta.started_at_ms);
        self.report = Some(report.clone());
        if !self.is_running() {
            self.phase = Phase::Finished;
        }
        cx.notify();
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = &self.cancel {
            c.cancel();
            self.logs.push(LogLine { file: String::new(), text: "已请求停止：当前请求结束后中止".into(), notice: true });
            cx.notify();
        }
    }

    /// 启动任务。`workflow_steps` 为工作流预置的步骤（开始前即可展示完整 stepper）
    pub fn start(&mut self, root: PathBuf, job: RunJob, workflow_steps: Vec<StepView>, cx: &mut Context<Self>) {
        if self.is_running() {
            return;
        }
        let cancel = CancelToken::new();
        let send = job.is_send();
        self.phase = Phase::Running;
        self.started_at = Some(Instant::now());
        self.current = None;
        self.cancel = Some(cancel.clone());
        // 单发只更新响应面板，结果表保持原运行不变
        if !send {
            self.command = job.command_line();
            self.rows.clear();
            self.logs.clear();
            self.planned = 0;
            self.source = ResultsSource::Live;
            self.report = None;
            self.workflow = match &job {
                RunJob::Workflow { name, .. } => Some(name.clone()),
                _ => None,
            };
            self.steps = workflow_steps;
        }
        self.job = Some(job.clone());

        let (tx, mut rx) = mpsc::unbounded::<Msg>();
        let event_tx = tx.clone();
        let hook: EventHook = Arc::new(move |ev: &RunEvent| {
            let _ = event_tx.unbounded_send(Msg::Event(ev.clone()));
        });
        // libcurl 与 QuickJS 均为阻塞调用：独立线程执行，不占用 GPUI 执行器
        std::thread::spawn(move || {
            let out = match job {
                RunJob::Run(req) => {
                    session::run(&req.to_spec(&root, "desktop", true, Some(cancel)), Some(hook), |_, _, _| {}).map(|o| (o.results, o.report))
                }
                RunJob::Send { path, rel, env } => {
                    session::run(&session::send_spec(path, &rel, Some(env), "desktop", Some(cancel)), Some(hook), |_, _, _| {})
                        .map(|o| (o.results, o.report))
                }
                RunJob::Workflow { name, env, dry_run } => {
                    let spec = WorkflowSpec {
                        root,
                        name,
                        env: Some(env),
                        dry_run,
                        verbose: false,
                        send_mode: true,
                        source: "desktop".into(),
                        record_history: true,
                        cancel: Some(cancel),
                    };
                    workflow::run(&spec, Some(hook), |_| {}).map(|o| (o.results, o.report))
                }
            };
            let _ = tx.unbounded_send(Msg::Done(out));
        });

        self._task = Some(cx.spawn(async move |this, cx| {
            while let Some(msg) = rx.next().await {
                let alive = this.update(cx, |state, cx| state.apply(msg, cx)).is_ok();
                if !alive {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn apply(&mut self, msg: Msg, cx: &mut Context<Self>) {
        let send = self.job.as_ref().is_some_and(RunJob::is_send);
        match msg {
            Msg::Event(ev) => match ev {
                RunEvent::RunStarted { planned, .. } => {
                    if !send {
                        self.planned = planned;
                    }
                }
                RunEvent::StepStarted { index, label, run, planned, .. } => {
                    if let Some(s) = self.steps.get_mut(index) {
                        s.state = StepState::Running;
                        s.planned = planned;
                    } else {
                        self.steps.push(StepView { label, run, planned, state: StepState::Running, passed: 0, failed: 0, skipped: 0 });
                    }
                }
                RunEvent::StepFinished { index, passed, failed, skipped } => {
                    if let Some(s) = self.steps.get_mut(index) {
                        s.state = if failed > 0 { StepState::Failed } else { StepState::Passed };
                        s.passed = passed;
                        s.failed = failed;
                        s.skipped = skipped;
                    }
                }
                RunEvent::CaseStarted { file, .. } => self.current = Some(file),
                RunEvent::Log { file, line } => self.logs.push(LogLine { file, text: line, notice: false }),
                RunEvent::Notice { message } => self.logs.push(LogLine { file: String::new(), text: message, notice: true }),
                RunEvent::CaseFinished { result } => {
                    if !send {
                        self.rows.push(result);
                    }
                }
                RunEvent::RunFinished { .. } => {}
            },
            Msg::Done(out) => {
                if !send {
                    self.elapsed_ms = self.started_at.map(|t| t.elapsed().as_millis() as u64).unwrap_or(0);
                }
                self.current = None;
                self.cancel = None;
                // 未执行的步骤（前序失败 / 取消 / 出错）
                for s in &mut self.steps {
                    if matches!(s.state, StepState::Pending | StepState::Running) {
                        s.state = StepState::NotRun;
                    }
                }
                match out {
                    Ok((results, report)) => {
                        let dry_run = report.meta.as_ref().is_some_and(|m| m.dry_run);
                        let mut touched: Vec<String> = vec![];
                        let at = requestbaby_core::history::now_ms();
                        // dry-run 未发送请求：不覆盖响应面板中的真实响应
                        for r in results.into_iter().filter(|_| !dry_run) {
                            // 数据驱动多实例：优先保留失败实例
                            let keep_existing =
                                touched.contains(&r.rel) && self.responses.get(&r.rel).is_some_and(|(_, e)| !e.passed());
                            if !keep_existing {
                                touched.push(r.rel.clone());
                                self.responses.insert(r.rel.clone(), (at, r));
                            }
                        }
                        self.phase = Phase::Finished;
                        let (failed, total) = (report.failed, report.total);
                        if !send {
                            self.report = Some(report);
                        }
                        for file in touched {
                            cx.emit(RunStateEvent::CaseDone { file });
                        }
                        cx.emit(RunStateEvent::Finished { send, workflow: self.workflow.clone(), failed, total, dry_run });
                    }
                    Err(e) => {
                        let msg = format!("{e:#}");
                        self.phase = Phase::Failed(msg.clone());
                        cx.emit(RunStateEvent::Error(msg));
                    }
                }
            }
        }
        cx.notify();
    }
}
