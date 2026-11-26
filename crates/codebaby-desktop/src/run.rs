//! 运行控制：在后台线程执行 core 的 `session::run` / `workflow::run` / `verify::run`，
//! 通过与 CLI `--json-stream` 相同的 [`RunEvent`] 协议把进度流回界面。
use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::{Context, EventEmitter, Task};
use codebaby_core::report::{CaseReport, RunReport, StepStatus};
use codebaby_core::runner::{CancelToken, CaseResult, EventHook, RunEvent, SessionState};
use codebaby_core::session::{self, RunRequest};
use codebaby_core::verify::{self, VerifySpec};
use codebaby_core::workflow::{self, WorkflowSpec};
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
    /// 命名流程：写入共享历史
    Workflow { name: String, env: String, dry_run: bool },
    /// 完整验证（`codebaby verify`）：写入共享历史
    Verify { env: String },
}

impl RunJob {
    pub fn command_line(&self) -> String {
        match self {
            RunJob::Run(r) => r.command_line(),
            RunJob::Send { path, rel, env } => session::send_spec(path.clone(), rel, Some(env.clone()), "desktop", None).command,
            RunJob::Workflow { name, env, dry_run } => workflow::command_line(name, Some(env), *dry_run, false),
            RunJob::Verify { env } => verify::command_line(Some(env), None),
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

/// 流程步骤视图（运行中实时更新；历史运行从报告还原）
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

/// 正在运行的界面用例：已完成的步骤（截图已写入证据目录），一步一步实时增加
#[derive(Debug, Clone)]
pub struct LiveCase {
    pub file: String,
    /// 证据目录（`StepRecord::screenshot` 在其中）
    pub dir: String,
    pub steps: Vec<codebaby_core::engine::ui::StepRecord>,
    /// 最外层循环的进度（「第 12 轮，已运行 9 分 12 秒」）：循环里的步骤不逐条推送，长时间监控靠它看出还在跑
    pub progress: Option<String>,
}

pub enum RunStateEvent {
    /// 界面用例刚完成一步（实时展示用，内容见 [`RunState::live`]）
    LiveStep { file: String, send: bool },
    /// 批量运行里一个用例结束（结果已进结果表）：正在实时展示它的面板应换成最终结论
    CaseFinished { file: String },
    /// 一次运行结束（含取消）；send 不写历史
    Finished { send: bool, workflow: Option<String>, verify: bool, blocked: Option<String>, failed: usize, total: usize, dry_run: bool },
    /// 单用例完成（结果回填响应面板）
    CaseDone { file: String },
    /// 单发前自动补跑了前置用例（core::session 的提示）
    Prerequisite(String),
    Error(String),
}

pub struct RunState {
    phase: Phase,
    job: Option<RunJob>,
    planned: usize,
    started_at: Option<Instant>,
    elapsed_ms: u64,
    current: Option<String>,
    live: Option<LiveCase>,
    rows: Vec<CaseReport>,
    steps: Vec<StepView>,
    workflow: Option<String>,
    /// 结果来自完整验证（steps 为各阶段）
    verify: bool,
    source: ResultsSource,
    /// 结果表当前展示的报告（导出用）
    report: Option<RunReport>,
    /// 完整结果（含完整响应体），按 file 存最近一次实例与完成时间
    responses: HashMap<String, (u64, CaseResult)>,
    logs: Vec<LogLine>,
    command: String,
    cancel: Option<CancelToken>,
    _task: Option<Task<()>>,
    /// 本次会话：之前运行 / 发送捕获的变量与 Cookie，「发送」时带上（单独调试链路中的某一步）
    session: SessionState,
}

/// 任务结果：用例结果 + 报告 + 结束时的会话状态（verify 不回传）
type JobOutput = (Vec<CaseResult>, RunReport, Option<SessionState>);

enum Msg {
    Event(RunEvent),
    Done(anyhow::Result<JobOutput>),
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
            live: None,
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
            verify: false,
            session: SessionState::default(),
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

    /// 正在运行的界面用例及其已完成的步骤
    pub fn live(&self) -> Option<&LiveCase> {
        self.live.as_ref()
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

    pub fn is_verify(&self) -> bool {
        self.verify
    }

    pub fn session(&self) -> &SessionState {
        &self.session
    }

    pub fn clear_session(&mut self, cx: &mut Context<Self>) {
        self.session = SessionState::default();
        cx.notify();
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

    /// 清空结果区（运行记录被清理光后：不再展示已不存在的那次运行）。运行中不动
    pub fn clear_results(&mut self, cx: &mut Context<Self>) {
        if self.is_running() {
            return;
        }
        self.rows.clear();
        self.steps.clear();
        self.logs.clear();
        self.workflow = None;
        self.verify = false;
        self.source = ResultsSource::Live;
        self.command.clear();
        self.planned = 0;
        self.elapsed_ms = 0;
        self.report = None;
        self.current = None;
        self.live = None;
        self.phase = Phase::Idle;
        cx.notify();
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
        self.verify = meta.verify;
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

    /// 启动任务。`workflow_steps` 为流程预置的步骤（开始前即可展示完整 stepper）
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
            self.verify = matches!(job, RunJob::Verify { .. });
            self.steps = workflow_steps;
        }
        self.job = Some(job.clone());

        let (tx, mut rx) = mpsc::unbounded::<Msg>();
        let event_tx = tx.clone();
        let hook: EventHook = Arc::new(move |ev: &RunEvent| {
            let _ = event_tx.unbounded_send(Msg::Event(ev.clone()));
        });
        let seed = (!self.session.is_empty()).then(|| self.session.clone());
        // libcurl 与 QuickJS 均为阻塞调用：独立线程执行，不占用 GPUI 执行器
        std::thread::spawn(move || {
            let out = match job {
                RunJob::Run(req) => session::run(&req.to_spec(&root, "desktop", true, Some(cancel)), Some(hook), |_, _, _| {})
                    .map(|o| (o.results, o.report, Some(o.state))),
                // 单发带上本次会话捕获的变量与 Cookie：可单独调试链路中的任意一步
                RunJob::Send { path, rel, env } => {
                    let mut spec = session::send_spec(path, &rel, Some(env), "desktop", Some(cancel));
                    spec.seed = seed;
                    session::run(&spec, Some(hook), |_, _, _| {}).map(|o| (o.results, o.report, Some(o.state)))
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
                        artifacts: None,
                        skip_needs: false,
                        base: None,
                    };
                    workflow::run(&spec, Some(hook), |_| {}).map(|o| (o.results, o.report, Some(o.state)))
                }
                RunJob::Verify { env } => {
                    let spec = VerifySpec {
                        root,
                        env: Some(env),
                        selection: Default::default(),
                        output: None,
                        send_mode: true,
                        source: "desktop".into(),
                        record_history: true,
                        cancel: Some(cancel),
                    };
                    verify::run(&spec, Some(hook), |_| {}).map(|o| (o.results, o.report, None))
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
                RunEvent::CaseStarted { file, .. } => {
                    self.current = Some(file);
                    self.live = None;
                }
                RunEvent::UiStep { file, dir, step } => {
                    if self.live.as_ref().is_none_or(|l| l.file != file) {
                        self.live = Some(LiveCase { file: file.clone(), dir, steps: vec![], progress: None });
                    }
                    if let Some(l) = self.live.as_mut() {
                        l.steps.push(step);
                    }
                    cx.emit(RunStateEvent::LiveStep { file, send });
                }
                RunEvent::LoopProgress { file, dir, text, .. } => {
                    if self.live.as_ref().is_none_or(|l| l.file != file) {
                        self.live = Some(LiveCase { file: file.clone(), dir, steps: vec![], progress: None });
                    }
                    if let Some(l) = self.live.as_mut() {
                        l.progress = Some(text);
                    }
                    cx.emit(RunStateEvent::LiveStep { file, send });
                }
                RunEvent::Log { file, line } => self.logs.push(LogLine { file, text: line, notice: false }),
                RunEvent::Notice { message } => {
                    if send && message.starts_with("先运行前置用例") {
                        cx.emit(RunStateEvent::Prerequisite(message.clone()));
                    }
                    self.logs.push(LogLine { file: String::new(), text: message, notice: true })
                }
                RunEvent::CaseFinished { result } => {
                    if !send {
                        let file = result.file.clone();
                        self.rows.push(result);
                        cx.emit(RunStateEvent::CaseFinished { file });
                    }
                }
                RunEvent::RunFinished { .. } => {}
            },
            Msg::Done(out) => {
                if !send {
                    self.elapsed_ms = self.started_at.map(|t| t.elapsed().as_millis() as u64).unwrap_or(0);
                }
                self.current = None;
                self.live = None;
                self.cancel = None;
                // 未执行的步骤（前序失败 / 取消 / 出错）
                for s in &mut self.steps {
                    if matches!(s.state, StepState::Pending | StepState::Running) {
                        s.state = StepState::NotRun;
                    }
                }
                match out {
                    Ok((results, report, state)) => {
                        let dry_run = report.meta.as_ref().is_some_and(|m| m.dry_run);
                        if let Some(state) = state.filter(|_| !dry_run) {
                            self.session.merge(&state);
                        }
                        let mut touched: Vec<String> = vec![];
                        let at = codebaby_core::history::now_ms();
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
                        let blocked = report.blocked.clone();
                        if !send {
                            self.report = Some(report);
                        }
                        for file in touched {
                            cx.emit(RunStateEvent::CaseDone { file });
                        }
                        cx.emit(RunStateEvent::Finished {
                            send,
                            workflow: self.workflow.clone(),
                            verify: self.verify,
                            blocked,
                            failed,
                            total,
                            dry_run,
                        });
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
