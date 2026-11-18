//! 流程编排：`requestbaby.json` 中命名的多阶段运行。
//!
//! 各步骤共享同一个 Runner（运行时变量跨步骤自动传递），步骤内 fail-fast，
//! 任一步骤失败即中止后续步骤（dry-run 除外）。CLI `workflow` 与 Desktop 共用本模块。
use crate::config::{load_strict, Project, Workflow, WorkflowStep};
use crate::history;
use crate::report::{RunMeta, RunReport, StepReport, StepStatus};
use crate::runner::{self, discover, CancelToken, CaseResult, EventHook, RunEvent, RunOptions, Runner, SessionState};
use crate::session::quote;
use anyhow::{anyhow, Context, Result};
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
    /// 结束时的会话状态（各步骤累计捕获的变量 + Cookie）
    pub state: SessionState,
}

/// 读取项目配置中的流程；不存在时列出已定义名称
pub fn load(root: &Path, name: &str) -> Result<(Project, Workflow)> {
    let pf = discover::project_file(root).ok_or_else(|| anyhow!("缺少 requestbaby.json（{}）", root.display()))?;
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

/// 流程实际运行的环境：固定了 env 用它（显式请求其他环境 → 错误）；否则请求的环境，再缺省 defaultEnv
pub fn resolve_env(project: &Project, key: &str, wf: &Workflow, requested: Option<&str>) -> Result<String> {
    match (wf.env.as_deref(), requested) {
        (Some(pinned), Some(req)) if pinned != req => Err(anyhow!(
            "流程 {key} 固定在环境 {pinned}（requestbaby.json workflows.{key}.env），不能用 -e {req} 运行：去掉 -e，或改流程的 env"
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
    let display_name = if wf.name.is_empty() { spec.name.clone() } else { wf.name.clone() };

    if wf.steps.is_empty() {
        return Err(anyhow!("流程 {} 还没有步骤（requestbaby workflow step add {} <目录或用例>）", spec.name, spec.name));
    }
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
    let state = runner.state();
    let meta = RunMeta {
        id,
        source: spec.source.clone(),
        // 固定环境的流程：等价命令不带 -e（带了反而会被拒绝）
        command: command_line(&spec.name, wf.env.is_none().then_some(env.as_str()), spec.dry_run),
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
// 直接编辑 requestbaby.json 的 workflows 字段，其余字段与顺序原样保留。

/// 与 `requestbaby workflow <子命令>` 冲突的名称不能作为流程 key
pub const RESERVED_KEYS: &[&str] = &["list", "show", "new", "rename", "cp", "rm", "step", "env", "help"];

/// 流程 key：CLI 参数直接使用，禁止空白 / 路径分隔 / 保留字
pub fn check_key(key: &str) -> Result<()> {
    if key.is_empty() {
        return Err(anyhow!("流程 key 不能为空"));
    }
    if key.chars().any(|c| c.is_whitespace() || matches!(c, '/' | '\\' | '"' | '\'')) {
        return Err(anyhow!("流程 key 不能包含空白、引号或路径分隔符: {key}"));
    }
    if RESERVED_KEYS.contains(&key) {
        return Err(anyhow!("{key} 是 requestbaby workflow 的子命令名，不能作为流程 key"));
    }
    Ok(())
}

/// 全部流程（requestbaby.json 中的顺序）
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
    edit(root, |map| map.shift_remove(key).map(|_| ()).ok_or_else(|| anyhow!("流程 {key} 不存在")))
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
    let pf = discover::project_file(root).ok_or_else(|| anyhow!("缺少 requestbaby.json（{}）", root.display()))?;
    let text = std::fs::read_to_string(&pf).with_context(|| format!("读取失败: {}", pf.display()))?;
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

/// 改写 requestbaby.json 顶层对象（其他字段与顺序原样保留，按规范格式写回）
pub(crate) fn edit_project(root: &Path, f: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>) -> Result<()>) -> Result<()> {
    let mut doc = read_doc(root)?;
    f(doc.as_object_mut().ok_or_else(|| anyhow!("requestbaby.json 顶层必须是对象"))?)?;
    let pf = root.join(discover::PROJECT_FILE);
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
}
