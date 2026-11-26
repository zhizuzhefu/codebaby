//! 流程编排：`codebaby.json` 中命名的多阶段运行。
//!
//! 各步骤共享同一个 Runner（运行时变量跨步骤自动传递），步骤内 fail-fast，
//! 任一步骤失败即中止后续步骤（dry-run 除外）。CLI `workflow` 与 Desktop 共用本模块。
use crate::config::{load_strict, Project, Workflow, WorkflowStep};
use crate::history;
use crate::report::{RunMeta, RunReport, StepReport, StepStatus};
use crate::runner::{self, discover, CancelToken, CaseResult, EventHook, RunEvent, RunOptions, Runner, SessionState};
use crate::session::quote;
use anyhow::{anyhow, Context, Result};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub struct WorkflowSpec {
    /// 项目根
    pub root: PathBuf,
    /// 流程 key
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
    /// 证据目录（verify 内运行时用 verify 的目录）；None = 按本次运行 ID
    pub artifacts: Option<crate::runner::ArtifactsDir>,
    /// 不运行依赖的流程（`--no-needs`）：只跑本流程自己的步骤
    pub skip_needs: bool,
    /// 别的流程已经跑好的同一组依赖留下的登录态（verify 里依赖相同的业务流程共用同一次登录）。
    /// 只有依赖的流程集合完全一致才会采用，否则本流程从零开始自己跑依赖
    pub base: Option<SessionBase>,
}

/// 一组依赖流程跑完之后的登录态：`satisfied` 是这组依赖（含间接依赖）
#[derive(Debug, Clone, Default)]
pub struct SessionBase {
    pub state: SessionState,
    pub satisfied: BTreeSet<String>,
}

/// 展开后的一个步骤：来自哪个依赖流程（None = 本流程自己的）
#[derive(Debug, Clone)]
pub struct PlannedStep {
    pub from: Option<String>,
    pub step: WorkflowStep,
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
    /// 来自哪个依赖流程（key）；None = 本流程自己的步骤
    pub from: Option<String>,
    /// 以哪个用户运行（None = 默认用户）
    pub user: Option<String>,
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
    /// 结束时的会话状态（各步骤累计捕获的变量 + Cookie）
    pub state: SessionState,
    /// 依赖流程全部跑通后的登录态（还没有自己的步骤改动它）：verify 把它交给依赖相同的下一个流程。
    /// 没有依赖、依赖没跑通、或跳过了依赖时 satisfied 为空
    pub base: SessionBase,
}

/// 读取项目配置中的流程；不存在时列出已定义名称
pub fn load(root: &Path, name: &str) -> Result<(Project, Workflow)> {
    let pf = discover::project_file(root).ok_or_else(|| anyhow!("缺少 codebaby.json（{}）", root.display()))?;
    let project = load_strict::<Project>(&pf).context("项目配置错误")?;
    let wf = project.workflows.get(name).cloned().ok_or_else(|| {
        let defined: Vec<String> = project.workflows.keys().cloned().collect();
        anyhow!(
            "流程 {name} 不存在。已定义: {}",
            if defined.is_empty() { "（无）".to_string() } else { defined.join(", ") }
        )
    })?;
    Ok((project, wf))
}

/// 等价 CLI 命令
pub fn command_line(name: &str, env: Option<&str>, dry_run: bool, skip_needs: bool) -> String {
    let mut s = format!("codebaby workflow {}", quote(name));
    if let Some(e) = env {
        s.push_str(&format!(" -e {}", quote(e)));
    }
    if dry_run {
        s.push_str(" --dry-run");
    }
    if skip_needs {
        s.push_str(" --no-needs");
    }
    s
}

/// 流程的显示名（缺省用 key）
pub fn display_name(key: &str, wf: &Workflow) -> String {
    if wf.name.is_empty() {
        key.to_string()
    } else {
        wf.name.clone()
    }
}

/// 流程 `key` 依赖的全部流程（含间接依赖），按「先被依赖的在前」排好、去重，不含 key 自己。
/// 依赖的流程不存在、或依赖成环时报错
pub fn needs_closure(project: &Project, key: &str) -> Result<Vec<String>> {
    fn visit(project: &Project, key: &str, stack: &mut Vec<String>, done: &mut Vec<String>) -> Result<()> {
        if done.iter().any(|d| d == key) {
            return Ok(());
        }
        if let Some(pos) = stack.iter().position(|s| s == key) {
            let mut cycle: Vec<&str> = stack[pos..].iter().map(String::as_str).collect();
            cycle.push(key);
            return Err(anyhow!("流程依赖成环：{}", cycle.join(" → ")));
        }
        let wf = project.workflows.get(key).ok_or_else(|| anyhow!("流程 {key} 不存在"))?;
        stack.push(key.to_string());
        for need in &wf.needs {
            if !project.workflows.contains_key(need) {
                let defined: Vec<&str> = project.workflows.keys().map(String::as_str).collect();
                return Err(anyhow!("流程 {key} 依赖的流程 {need} 不存在。已定义: {}", defined.join(", ")));
            }
            visit(project, need, stack, done)?;
        }
        stack.pop();
        done.push(key.to_string());
        Ok(())
    }
    let mut done = vec![];
    visit(project, key, &mut vec![], &mut done)?;
    done.retain(|d| d != key);
    Ok(done)
}

/// 展开：依赖流程的步骤（先被依赖的在前）+ 本流程自己的步骤。`satisfied` 里的依赖本次已经跑过，不再展开
pub fn expand(project: &Project, key: &str, satisfied: &BTreeSet<String>, skip_needs: bool) -> Result<Vec<PlannedStep>> {
    let wf = project.workflows.get(key).ok_or_else(|| anyhow!("流程 {key} 不存在"))?;
    let mut out = vec![];
    let closure = needs_closure(project, key)?; // 即使跳过依赖也检查：配置错误要尽早暴露
    if !skip_needs {
        for dep in closure.iter().filter(|d| !satisfied.contains(*d)) {
            out.extend(project.workflows[dep].steps.iter().map(|s| PlannedStep { from: Some(dep.clone()), step: s.clone() }));
        }
    }
    out.extend(wf.steps.iter().map(|s| PlannedStep { from: None, step: s.clone() }));
    Ok(out)
}

/// 步骤展示名：依赖流程的步骤前面带上流程名（`登录 › 打开登录页`），指定了用户的后面带上用户
fn planned_label(project: &Project, p: &PlannedStep) -> String {
    let mut label = step_label(&p.step);
    if let Some(from) = &p.from {
        let name = project.workflows.get(from).map(|w| display_name(from, w)).unwrap_or_else(|| from.clone());
        label = format!("{name} › {label}");
    }
    if let Some(u) = &p.step.as_user {
        label.push_str(&format!("（用户 {u}）"));
    }
    label
}

/// 流程运行前的步骤清单（含依赖流程的步骤）：桌面端据此一次画出完整的进度条
pub fn plan_steps(root: &Path, name: &str, skip_needs: bool) -> Result<Vec<(String, WorkflowStep)>> {
    let pf = discover::project_file(root).ok_or_else(|| anyhow!("缺少 codebaby.json（{}）", root.display()))?;
    let project = load_strict::<Project>(&pf).context("项目配置错误")?;
    Ok(expand(&project, name, &BTreeSet::new(), skip_needs)?.into_iter().map(|p| (planned_label(&project, &p), p.step)).collect())
}

pub fn step_label(step: &WorkflowStep) -> String {
    if step.name.is_empty() {
        step.run.clone()
    } else {
        step.name.clone()
    }
}

/// 流程实际运行的环境：固定了 env 用它（显式请求其他环境 → 错误）；否则请求的环境，再缺省 defaultEnv
pub fn resolve_env(project: &Project, key: &str, wf: &Workflow, requested: Option<&str>) -> Result<String> {
    match (wf.env.as_deref(), requested) {
        (Some(pinned), Some(req)) if pinned != req => Err(anyhow!(
            "流程 {key} 固定在环境 {pinned}（codebaby.json workflows.{key}.env），不能用 -e {req} 运行：去掉 -e，或改流程的 env"
        )),
        (Some(pinned), _) => Ok(pinned.to_string()),
        (None, Some(req)) => Ok(req.to_string()),
        (None, None) => Ok(project.default_env.clone()),
    }
}

pub fn run(spec: &WorkflowSpec, hook: Option<EventHook>, mut on_step: impl FnMut(StepPhase<'_>)) -> Result<WorkflowOutcome> {
    let (project, wf) = load(&spec.root, &spec.name)?;
    let env = resolve_env(&project, &spec.name, &wf, spec.env.as_deref())?;
    let ctx = runner::load_project_ctx(&spec.root, &env)?;
    let display_name = display_name(&spec.name, &wf);

    let closure = needs_closure(&project, &spec.name)?;
    let closure_set: BTreeSet<String> = closure.iter().cloned().collect();
    // 依赖相同的流程共用同一份登录态；依赖不一样就不采用（否则会带进不属于本流程依赖的登录态）
    let reuse = spec.base.as_ref().filter(|b| !spec.skip_needs && !closure_set.is_empty() && b.satisfied == closure_set);
    let incoming = reuse.cloned().unwrap_or_default();
    // 依赖的流程要在同一个环境里跑
    for dep in &closure {
        if let Some(pinned) = project.workflows[dep.as_str()].env.as_deref().filter(|e| *e != env) {
            return Err(anyhow!("流程 {} 依赖的流程 {dep} 固定在环境 {pinned}，与本次环境 {env} 不一致：去掉依赖流程的 env，或把本流程也固定到 {pinned}", spec.name));
        }
    }
    let expanded = expand(&project, &spec.name, &incoming.satisfied, spec.skip_needs)?;
    if expanded.is_empty() {
        return Err(anyhow!("流程 {} 还没有步骤（codebaby workflow step add {} <目录或用例>）", spec.name, spec.name));
    }
    // 先发现全部步骤：配置错误在执行任何请求前暴露
    let total = expanded.len();
    let mut planned = vec![];
    for (index, p) in expanded.iter().enumerate() {
        let step = &p.step;
        let target = spec.root.join(&step.run);
        let whose = p.from.as_deref().map(|f| format!("依赖流程 {f} 的")).unwrap_or_default();
        if !target.exists() {
            return Err(anyhow!("{whose}步骤 {} 的路径不存在: {}", index + 1, step.run));
        }
        let cases = discover::discover(std::slice::from_ref(&target))
            .with_context(|| format!("步骤 {} 发现用例失败", step.run))?;
        let info = StepInfo {
            index,
            total,
            label: planned_label(&project, p),
            run: step.run.clone(),
            tags: step.tags.clone(),
            name_filter: step.name_filter.clone(),
            planned: cases.len(),
            from: p.from.clone(),
            user: step.as_user.clone(),
        };
        planned.push((info, cases));
    }
    // 依赖流程的步骤数：它们全部跑通的那一刻，登录态就绪
    let needs_steps = planned.iter().filter(|(i, _)| i.from.is_some()).count();
    // 本流程依赖的登录态：采用别人跑好的；或自己跑完依赖后得到（跳过依赖时没有）
    let mut base = if spec.skip_needs { SessionBase::default() } else { SessionBase { state: incoming.state.clone(), satisfied: if reuse.is_some() { closure_set.clone() } else { BTreeSet::new() } } };

    let emit = |ev: RunEvent| {
        if let Some(h) = &hook {
            h(&ev);
        }
    };
    let started_at_ms = history::now_ms();
    // 对比基准：执行前读取
    let baseline = (!spec.dry_run).then(|| history::last_status(&spec.root, &env));
    let id = history::new_run_id(&spec.source, started_at_ms);
    emit(RunEvent::RunStarted {
        id: id.clone(),
        env: env.clone(),
        source: spec.source.clone(),
        planned: planned.iter().map(|(i, _)| i.planned).sum(),
    });

    let mut runner = Runner::new(ctx);
    if reuse.is_some() {
        runner.seed(incoming.state.clone());
    }
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
            artifacts: if spec.dry_run { None } else { Some(spec.artifacts.clone().unwrap_or_else(|| history::artifacts_for(&spec.root, &id, spec.record_history))) },
            ..Default::default()
        };
        runner.switch_user(info.user.as_deref());
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
        // 依赖流程全部跑通：此刻的登录态交给后面的流程共用
        if needs_steps > 0 && info.index + 1 == needs_steps && (!failed || spec.dry_run) {
            base = SessionBase { state: runner.state(), satisfied: closure_set.clone() };
        }
    }

    let notices = runner.take_notices();
    let state = runner.state();
    let meta = RunMeta {
        id,
        source: spec.source.clone(),
        // 固定环境的流程：等价命令不带 -e（带了反而会被拒绝）
        command: command_line(&spec.name, wf.env.is_none().then_some(env.as_str()), spec.dry_run, spec.skip_needs),
        env: env.clone(),
        started_at_ms,
        finished_at_ms: history::now_ms(),
        dry_run: spec.dry_run,
        cancelled: cancelled(),
        workflow: Some(spec.name.clone()),
        verify: false,
        git: (!spec.dry_run).then(|| history::git_info(&spec.root)).flatten(),
    };
    let mut report = RunReport::new(&all, true, Some(meta));
    report.steps = step_reports;
    report.compare = baseline.and_then(|b| history::compare(&b, &report));
    if let Some(c) = report.compare.as_mut() {
        c.flaky = history::flaky(&spec.root, &env, &RunReport::new(&all, false, report.meta.clone()));
    }
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
        state,
        base,
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

// ---------------------------------------------------------------- 管理（增删改查）
//
// CLI `workflow new / show / rename / cp / rm / step …` 与 Desktop 流程编辑器共用：
// 直接编辑 codebaby.json 的 workflows 字段，其余字段与顺序原样保留。

/// 与 `codebaby workflow <子命令>` 冲突的名称不能作为流程 key
pub const RESERVED_KEYS: &[&str] = &["list", "show", "new", "rename", "cp", "rm", "step", "env", "needs", "help"];

/// 流程 key：CLI 参数直接使用，禁止空白 / 路径分隔 / 保留字
pub fn check_key(key: &str) -> Result<()> {
    if key.is_empty() {
        return Err(anyhow!("流程 key 不能为空"));
    }
    if key.chars().any(|c| c.is_whitespace() || matches!(c, '/' | '\\' | '"' | '\'')) {
        return Err(anyhow!("流程 key 不能包含空白、引号或路径分隔符: {key}"));
    }
    if RESERVED_KEYS.contains(&key) {
        return Err(anyhow!("{key} 是 codebaby workflow 的子命令名，不能作为流程 key"));
    }
    Ok(())
}

/// 全部流程（codebaby.json 中的顺序）
pub fn list(root: &Path) -> Result<Vec<(String, Workflow)>> {
    let doc = read_doc(root)?;
    let Some(map) = doc.get("workflows").and_then(|w| w.as_object()) else { return Ok(vec![]) };
    map.iter()
        .map(|(k, v)| {
            serde_json::from_value::<Workflow>(v.clone()).map(|w| (k.clone(), w)).with_context(|| format!("流程 {k} 格式错误"))
        })
        .collect()
}

pub fn get(root: &Path, key: &str) -> Result<Workflow> {
    list(root)?.into_iter().find(|(k, _)| k == key).map(|(_, w)| w).ok_or_else(|| not_found(root, key))
}

/// 新建 / 保存 / 重命名：`original` 为编辑前的 key（新建为 None）。重命名保持原位置
pub fn save(root: &Path, original: Option<&str>, key: &str, wf: &Workflow) -> Result<()> {
    check_key(key)?;
    check_steps(root, wf)?;
    check_needs(root, original, key, wf)?;
    if let Some(env) = &wf.env {
        if !crate::config::env_path(root, env).exists() {
            return Err(anyhow!("环境不存在: {env}（envs/{env}.json）"));
        }
    }
    let value = serde_json::to_value(wf)?;
    edit(root, |map| {
        if original != Some(key) && map.contains_key(key) {
            return Err(anyhow!("流程 {key} 已存在"));
        }
        match original.filter(|o| map.contains_key(*o)) {
            Some(o) => {
                let old = std::mem::take(map);
                for (k, v) in old {
                    if k == o {
                        map.insert(key.to_string(), value.clone());
                    } else {
                        map.insert(k, v);
                    }
                }
                // 重命名：别的流程里对它的依赖一并改名
                if o != key {
                    for (_, v) in map.iter_mut() {
                        if let Some(needs) = v.get_mut("needs").and_then(|n| n.as_array_mut()) {
                            for n in needs.iter_mut().filter(|n| n.as_str() == Some(o)) {
                                *n = serde_json::Value::String(key.to_string());
                            }
                        }
                    }
                }
            }
            None if original.is_some() => return Err(anyhow!("流程 {} 不存在（可能已在外部被删除）", original.unwrap_or_default())),
            None => {
                map.insert(key.to_string(), value);
            }
        }
        Ok(())
    })
}

pub fn delete(root: &Path, key: &str) -> Result<()> {
    let users: Vec<String> = list(root)?.into_iter().filter(|(k, w)| k != key && w.needs.iter().any(|n| n == key)).map(|(k, _)| k).collect();
    if !users.is_empty() {
        return Err(anyhow!("流程 {key} 被 {} 依赖：先从它们的依赖里去掉 {key}（codebaby workflow needs <流程> rm {key}）", users.join("、")));
    }
    edit(root, |map| map.shift_remove(key).map(|_| ()).ok_or_else(|| anyhow!("流程 {key} 不存在")))
}

/// 增加一个依赖流程（已有则不变），返回新的依赖列表
pub fn add_need(root: &Path, key: &str, need: &str) -> Result<Vec<String>> {
    let mut wf = get(root, key)?;
    if !wf.needs.iter().any(|n| n == need) {
        wf.needs.push(need.to_string());
    }
    save(root, Some(key), key, &wf)?;
    Ok(wf.needs)
}

/// 去掉一个依赖流程，返回新的依赖列表
pub fn remove_need(root: &Path, key: &str, need: &str) -> Result<Vec<String>> {
    let mut wf = get(root, key)?;
    if !wf.needs.iter().any(|n| n == need) {
        return Err(anyhow!("流程 {key} 没有依赖 {need}（现有依赖：{}）", if wf.needs.is_empty() { "无".to_string() } else { wf.needs.join("、") }));
    }
    wf.needs.retain(|n| n != need);
    save(root, Some(key), key, &wf)?;
    Ok(wf.needs)
}

/// 复制为新 key（显示名追加「副本」）
pub fn duplicate(root: &Path, key: &str, new_key: &str) -> Result<()> {
    let mut wf = get(root, key)?;
    if !wf.name.is_empty() {
        wf.name.push_str(" 副本");
    }
    save(root, None, new_key, &wf)
}

/// 插入步骤（`at` 为 0 起的位置，缺省追加），返回插入位置
pub fn add_step(root: &Path, key: &str, step: WorkflowStep, at: Option<usize>) -> Result<usize> {
    let mut wf = get(root, key)?;
    let ix = at.unwrap_or(wf.steps.len()).min(wf.steps.len());
    wf.steps.insert(ix, step);
    save(root, Some(key), key, &wf)?;
    Ok(ix)
}

pub fn remove_step(root: &Path, key: &str, index: usize) -> Result<WorkflowStep> {
    let mut wf = get(root, key)?;
    if index >= wf.steps.len() {
        return Err(anyhow!("流程 {key} 只有 {} 个步骤", wf.steps.len()));
    }
    let step = wf.steps.remove(index);
    save(root, Some(key), key, &wf)?;
    Ok(step)
}

pub fn move_step(root: &Path, key: &str, from: usize, to: usize) -> Result<()> {
    let mut wf = get(root, key)?;
    let n = wf.steps.len();
    if from >= n || to >= n {
        return Err(anyhow!("步骤位置超出范围（流程 {key} 共 {n} 步）"));
    }
    let step = wf.steps.remove(from);
    wf.steps.insert(to, step);
    save(root, Some(key), key, &wf)
}

/// 保存前检查依赖：依赖的流程要存在、不能依赖自己、不能成环
fn check_needs(root: &Path, original: Option<&str>, key: &str, wf: &Workflow) -> Result<()> {
    if wf.needs.is_empty() {
        return Ok(());
    }
    let pf = discover::project_file(root).ok_or_else(|| anyhow!("缺少 codebaby.json（{}）", root.display()))?;
    let mut project = load_strict::<Project>(&pf).context("项目配置错误")?;
    if let Some(o) = original.filter(|o| *o != key) {
        if let Some(old) = project.workflows.remove(o) {
            project.workflows.insert(key.to_string(), old);
        }
        // 重命名后别的流程对它的依赖也会跟着改名（见 save），这里按改名后的状态检查
        for w in project.workflows.values_mut() {
            for n in w.needs.iter_mut().filter(|n| *n == o) {
                *n = key.to_string();
            }
        }
    }
    project.workflows.insert(key.to_string(), wf.clone());
    for n in &wf.needs {
        if n == key {
            return Err(anyhow!("流程 {key} 不能依赖自己"));
        }
    }
    needs_closure(&project, key).map(|_| ())
}

fn check_steps(root: &Path, wf: &Workflow) -> Result<()> {
    for (i, s) in wf.steps.iter().enumerate() {
        if s.run.trim().is_empty() {
            return Err(anyhow!("步骤 {} 未选择运行目标", i + 1));
        }
        if !root.join(&s.run).exists() {
            return Err(anyhow!("步骤 {} 的运行目标不存在: {}", i + 1, s.run));
        }
    }
    Ok(())
}

fn not_found(root: &Path, key: &str) -> anyhow::Error {
    let defined: Vec<String> = list(root).map(|l| l.into_iter().map(|(k, _)| k).collect()).unwrap_or_default();
    anyhow!("流程 {key} 不存在。已定义: {}", if defined.is_empty() { "（无）".to_string() } else { defined.join(", ") })
}

fn read_doc(root: &Path) -> Result<serde_json::Value> {
    let pf = discover::project_file(root).ok_or_else(|| anyhow!("缺少 codebaby.json（{}）", root.display()))?;
    read_project_file(&pf)
}

fn read_project_file(pf: &Path) -> Result<serde_json::Value> {
    let text = std::fs::read_to_string(pf).with_context(|| format!("读取失败: {}", pf.display()))?;
    serde_json::from_str(&text).with_context(|| format!("解析失败: {}", pf.display()))
}

fn edit(root: &Path, f: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>) -> Result<()>) -> Result<()> {
    edit_project(root, |obj| {
        let wfs = obj.entry("workflows").or_insert_with(|| serde_json::Value::Object(Default::default()));
        if !wfs.is_object() {
            *wfs = serde_json::Value::Object(Default::default());
        }
        f(wfs.as_object_mut().expect("刚确保为对象"))
    })
}

/// 改写 codebaby.json 顶层对象（其他字段与顺序原样保留，按规范格式写回）
pub(crate) fn edit_project(root: &Path, f: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>) -> Result<()>) -> Result<()> {
    let pf = discover::project_file(root).ok_or_else(|| anyhow!("缺少 codebaby.json（{}）", root.display()))?;
    let mut doc = read_project_file(&pf)?;
    f(doc.as_object_mut().ok_or_else(|| anyhow!("codebaby.json 顶层必须是对象"))?)?;
    crate::fsx::write(&pf, crate::fmt::canonical_project_text(&doc)).with_context(|| format!("写入失败: {}", pf.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        crate::scaffold::init_project(dir.path()).unwrap();
        dir
    }

    fn step(run: &str) -> WorkflowStep {
        WorkflowStep { run: run.into(), ..Default::default() }
    }

    #[test]
    fn crud_round_trip_keeps_other_fields_and_order() {
        let dir = project();
        let root = dir.path();
        let wf = Workflow { name: "冒烟".into(), steps: vec![step("collections/demo")], ..Default::default() };
        save(root, None, "smoke", &wf).unwrap();
        save(root, None, "full", &Workflow { steps: vec![step("collections")], ..Default::default() }).unwrap();
        assert!(save(root, None, "smoke", &wf).unwrap_err().to_string().contains("已存在"));
        // 重命名保持位置
        save(root, Some("smoke"), "smoke2", &wf).unwrap();
        let keys: Vec<String> = list(root).unwrap().into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["smoke2", "full"]);
        // 步骤增删移
        add_step(root, "smoke2", WorkflowStep { name: "登录".into(), ..step("collections/demo/02-login.json") }, Some(0)).unwrap();
        move_step(root, "smoke2", 0, 1).unwrap();
        assert_eq!(get(root, "smoke2").unwrap().steps[1].name, "登录");
        remove_step(root, "smoke2", 1).unwrap();
        duplicate(root, "smoke2", "smoke3").unwrap();
        assert_eq!(get(root, "smoke3").unwrap().name, "冒烟 副本");
        delete(root, "smoke2").unwrap();
        assert!(get(root, "smoke2").is_err());
        // 其他字段原样保留
        let doc = read_doc(root).unwrap();
        assert_eq!(doc["defaultEnv"], "dev");
        assert!(doc["workflows"]["full"]["steps"][0].get("tags").is_none(), "空字段不写出");
    }

    #[test]
    fn rejects_bad_keys_and_missing_targets() {
        let dir = project();
        let wf = Workflow { steps: vec![step("collections/nope")], ..Default::default() };
        assert!(save(dir.path(), None, "x", &wf).unwrap_err().to_string().contains("不存在"));
        assert!(check_key("list").is_err());
        assert!(check_key("a b").is_err());
        assert!(check_key("回归-1").is_ok());
    }

    #[test]
    fn pinned_env_wins_and_conflicting_e_is_rejected() {
        let dir = project();
        let root = dir.path();
        let project = load_strict::<Project>(&root.join(discover::PROJECT_FILE)).unwrap();
        let free = Workflow { steps: vec![step("collections/demo")], ..Default::default() };
        assert_eq!(resolve_env(&project, "free", &free, None).unwrap(), "dev");
        assert_eq!(resolve_env(&project, "free", &free, Some("test")).unwrap(), "test");
        let pinned = Workflow { env: Some("dev".into()), ..free.clone() };
        assert_eq!(resolve_env(&project, "fx", &pinned, None).unwrap(), "dev");
        assert_eq!(resolve_env(&project, "fx", &pinned, Some("dev")).unwrap(), "dev");
        assert!(resolve_env(&project, "fx", &pinned, Some("test")).unwrap_err().to_string().contains("固定在环境 dev"));
        // 固定到不存在的环境：保存时拒绝
        let bad = Workflow { env: Some("nope".into()), ..free };
        assert!(save(root, None, "bad", &bad).unwrap_err().to_string().contains("环境不存在"));
        save(root, None, "fx", &pinned).unwrap();
        assert_eq!(read_doc(root).unwrap()["workflows"]["fx"]["env"], "dev");
    }

    fn wf(needs: &[&str], runs: &[&str]) -> Workflow {
        Workflow { needs: needs.iter().map(|n| n.to_string()).collect(), steps: runs.iter().map(|r| step(r)).collect(), ..Default::default() }
    }

    /// 依赖：先被依赖的在前、去重；缺失与成环都有明确的错误
    #[test]
    fn needs_expand_in_dependency_order_and_dedupe() {
        let dir = project();
        let root = dir.path();
        std::fs::create_dir_all(root.join("collections/a")).unwrap();
        save(root, None, "login", &wf(&[], &["collections/a"])).unwrap();
        save(root, None, "admin", &wf(&["login"], &["collections"])).unwrap();
        save(root, None, "biz", &wf(&["login", "admin"], &["collections/a"])).unwrap();
        let pf = discover::project_file(root).unwrap();
        let project = load_strict::<Project>(&pf).unwrap();
        assert_eq!(needs_closure(&project, "biz").unwrap(), ["login", "admin"], "login 只出现一次，且在 admin 前");
        let planned = expand(&project, "biz", &BTreeSet::new(), false).unwrap();
        let from: Vec<Option<&str>> = planned.iter().map(|p| p.from.as_deref()).collect();
        assert_eq!(from, [Some("login"), Some("admin"), None]);
        // 已经跑过的依赖不再展开；跳过依赖只剩自己的步骤
        let done: BTreeSet<String> = ["login".to_string()].into();
        assert_eq!(expand(&project, "biz", &done, false).unwrap().len(), 2);
        assert_eq!(expand(&project, "biz", &BTreeSet::new(), true).unwrap().len(), 1);
        let labels = plan_steps(root, "biz", false).unwrap();
        assert_eq!(labels[0].0, "login › collections/a");
    }

    #[test]
    fn needs_reject_missing_self_and_cycles_and_follow_rename_and_delete() {
        let dir = project();
        let root = dir.path();
        std::fs::create_dir_all(root.join("collections/a")).unwrap();
        save(root, None, "login", &wf(&[], &["collections/a"])).unwrap();
        assert!(save(root, None, "x", &wf(&["nope"], &["collections/a"])).unwrap_err().to_string().contains("依赖的流程 nope 不存在"));
        assert!(save(root, None, "x", &wf(&["x"], &["collections/a"])).unwrap_err().to_string().contains("不能依赖自己"));
        save(root, None, "biz", &wf(&["login"], &["collections/a"])).unwrap();
        // 成环：login 反过来依赖 biz
        let err = save(root, Some("login"), "login", &wf(&["biz"], &["collections/a"])).unwrap_err().to_string();
        assert!(err.contains("成环") && err.contains("login → biz → login"), "{err}");
        // 被依赖的流程不能直接删；重命名时依赖跟着改
        assert!(delete(root, "login").unwrap_err().to_string().contains("被 biz 依赖"));
        save(root, Some("login"), "signin", &get(root, "login").unwrap()).unwrap();
        assert_eq!(get(root, "biz").unwrap().needs, ["signin"]);
        // 命令式增删依赖
        save(root, None, "other", &wf(&[], &["collections/a"])).unwrap();
        assert_eq!(add_need(root, "biz", "other").unwrap(), ["signin", "other"]);
        assert_eq!(remove_need(root, "biz", "signin").unwrap(), ["other"]);
        assert!(remove_need(root, "biz", "signin").unwrap_err().to_string().contains("没有依赖 signin"));
        delete(root, "signin").unwrap();
        // 只有依赖、没有自己步骤的流程是合法的
        save(root, None, "just-login", &wf(&["other"], &[])).unwrap();
        assert!(read_doc(root).unwrap()["workflows"]["just-login"]["steps"].as_array().unwrap().is_empty());
    }
}
