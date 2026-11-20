//! 工作区：标题栏 / 侧栏（集合树·流程·环境·项目）/ 中央编辑区 / 底部（运行结果）/ 状态栏。
//!
//! 所有能力都调用 requestbaby-core，与 CLI 同一实现：运行走 `session` / `workflow`，
//! 新建 / 重命名 / 删除走 `scaffold`，校验 / 格式化 / 自检走 `validate` / `fmt` / `doctor`。
//! 文件监听让两端共享同一份事实：AI 改了文件 → 树与编辑器刷新；终端跑了一次 → 结果立刻出现。
mod bottom;
mod envs;
mod proxy;
mod sidebar;
mod updates;

use crate::case_editor::{CaseEditor, CaseEditorEvent};
use crate::config_editors::{CollectionEditor, ConfigEditorEvent, ProjectEditor};
use crate::vars_panel::{VarsPanel, VarsPanelEvent};
use crate::workflow_editor::{WorkflowEditor, WorkflowEditorEvent};
use requestbaby_core::varstore::VarScope;
use crate::dialogs;
use crate::new_case_dialog::{self, NewCaseMode};
use sidebar::menu_item;
use crate::project::{CaseStatus, Project};
use crate::response::{ResponsePanel, ResponsePanelEvent};
use crate::style;
use crate::run::{RunJob, RunState, RunStateEvent, StepState, StepView};
use crate::run_dialog::{RunDialogInit, RunOptionsPanel};
use crate::shell::ShellRequest;
use crate::export_dialog::{CollectionItem, ExportPanel, RunChoice, Selection};
use crate::{CopyProjectPath, ExportResults, FindInTree, ManageEnvs, QuickOpen, RunAll, RunWithOptions, SaveCase, SendCase, StopRun, ToggleTheme, Verify};
use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, IndexPath, Sizable as _, TitleBar, WindowExt as _, h_flex,
    button::{Button, ButtonVariant, ButtonVariants as _},
    menu::{DropdownMenu as _, PopupMenuItem},
    notification::Notification,
    resizable::{h_resizable, resizable_panel, v_resizable},
    input::{InputEvent, InputState},
    select::{Select, SelectEvent, SelectState},
    tree::{TreeEvent, TreeState},
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, InteractiveElement as _,
    IntoElement, MouseButton, ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, Window, div,
    prelude::FluentBuilder as _, px,
};
use notify::Watcher as _;
use requestbaby_core::report::{CaseReport, RunReport};
use requestbaby_core::session::RunRequest;
use requestbaby_core::validate::ValidationIssue;
use requestbaby_core::{doctor, export, fmt, guide, scaffold, validate, verify, workflow};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 底部只有一个「运行结果」；脚本输出与配置问题仅在有内容时从结果页进入，随时可返回
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BottomTab {
    Results,
    Logs,
    Issues,
}

/// 中央编辑区可打开的文档
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    /// 用例（相对项目根）
    Case(String),
    /// 集合目录（相对项目根）的 collection.json
    Collection(String),
    /// 变量面板（None = 总览）
    Vars(Option<VarScope>),
    /// requestbaby.json
    Project,
    /// 流程编辑器（None = 新建）
    Workflow(Option<String>),
    /// 环境管理页
    Envs,
}

/// 中央编辑区当前内容。用例编辑器常驻（切去配置再回来不丢编辑），配置编辑器按需创建
enum Center {
    Case,
    Collection(String, Entity<CollectionEditor>),
    Vars(Entity<VarsPanel>),
    Project(Entity<ProjectEditor>),
    Workflow(Entity<WorkflowEditor>),
    Envs,
}

/// 项目级校验（`requestbaby validate` 同一实现）
#[derive(Default)]
struct Validation {
    issues: Vec<ValidationIssue>,
    running: bool,
    done: bool,
    error: Option<String>,
}

pub struct Workspace {
    project: Project,
    env: String,
    env_select: Entity<SelectState<Vec<SharedString>>>,
    tree: Entity<TreeState>,
    collapsed: HashSet<String>,
    /// 集合树筛选（名称 / 路径 / 方法 / URL）
    tree_filter: Entity<InputState>,
    center: Center,
    editor: Entity<CaseEditor>,
    response: Entity<ResponsePanel>,
    run: Entity<RunState>,
    bottom_tab: BottomTab,
    /// 运行结果：多端混合时可切到「时间线」（所有端按真实先后排在一起）
    results_timeline: bool,
    selected_row: Option<usize>,
    statuses: HashMap<String, CaseStatus>,
    seen_runs: HashSet<String>,
    validation: Validation,
    /// 运行配置对话框上次的参数
    last_request: Option<RunRequest>,
    focus_handle: FocusHandle,
    /// 检查到的新版本（状态栏提示）
    update: Option<requestbaby_core::update::Release>,
    update_busy: bool,
    _watcher: Option<notify::RecommendedWatcher>,
    _watch_task: Option<Task<()>>,
    _validate_task: Option<Task<()>>,
    _center_subscription: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ShellRequest> for Workspace {}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Workspace {
    pub fn new(root: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let project = Project::load(root);
        let env = pick_env(&project, None);
        let env_select = cx.new(|cx| SelectState::new(env_items(&project), env_index(&project, &env), window, cx));
        let tree = cx.new(|cx| TreeState::new(cx));
        let tree_filter = cx.new(|cx| InputState::new(window, cx).placeholder("筛选用例  ⌘P 快速打开").clean_on_escape());
        let editor = cx.new(|cx| CaseEditor::new(window, cx));
        editor.update(cx, |e, cx| e.set_env(env.clone(), cx));
        let response = cx.new(|cx| ResponsePanel::new(window, cx));
        let root = project.root().to_path_buf();
        response.update(cx, |p, _| p.set_root(Some(root)));
        let run = cx.new(|_| RunState::new());

        let subs = vec![
            cx.subscribe_in(&env_select, window, |this, _, ev: &SelectEvent<Vec<SharedString>>, window, cx| {
                if let SelectEvent::Confirm(Some(env)) = ev {
                    this.set_env(env.to_string(), window, cx);
                }
            }),
            cx.subscribe(&tree, |this, _, ev: &TreeEvent, _| match ev {
                TreeEvent::Expanded(id) => {
                    this.collapsed.remove(id.as_ref());
                }
                TreeEvent::Collapsed(id) => {
                    this.collapsed.insert(id.to_string());
                }
            }),
            cx.subscribe_in(&editor, window, |this, _, ev: &CaseEditorEvent, window, cx| match ev {
                CaseEditorEvent::Saved => this.after_save(window, cx),
                CaseEditorEvent::Send => this.send_current(window, cx),
                CaseEditorEvent::EditVar { name, defined_in } => this.edit_var(name.clone(), defined_in.clone(), window, cx),
                CaseEditorEvent::OpenVars => this.open(Target::Vars(None), window, cx),
            }),
            cx.subscribe_in(&run, window, Self::on_run_event),
            cx.subscribe_in(&response, window, Self::on_response_event),
            cx.subscribe(&tree_filter, |this, _, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.rebuild_tree(cx);
                    cx.notify();
                }
            }),
        ];

        let seen_runs = project.history().iter().filter_map(|r| r.meta.as_ref().map(|m| m.id.clone())).collect();
        let mut this = Self {
            project,
            env,
            env_select,
            tree,
            collapsed: HashSet::new(),
            tree_filter,
            center: Center::Case,
            editor,
            response,
            run,
            bottom_tab: BottomTab::Results,
            results_timeline: false,
            selected_row: None,
            statuses: HashMap::new(),
            seen_runs,
            validation: Validation::default(),
            last_request: None,
            focus_handle: cx.focus_handle(),
            update: None,
            update_busy: false,
            _watcher: None,
            _watch_task: None,
            _validate_task: None,
            _center_subscription: None,
            _subscriptions: subs,
        };
        this.statuses = this.project.latest_status();
        this.rebuild_tree(cx);
        this.start_watching(window, cx);
        this.revalidate(cx);
        this.auto_check_update(window, cx);
        // 启动时展示最近一次运行（无论来自 CLI 还是 Desktop）
        if let Some(latest) = this.project.history().first().cloned() {
            this.run.update(cx, |r, cx| r.show_report(&latest, cx));
        }
        if let Some(first) = this.project.cases().first().map(|c| c.rel.clone()) {
            this.open_now(Target::Case(first), window, cx);
        }
        this
    }

    /// 有未保存修改的文档（切换项目 / 退出前确认）
    pub fn dirty_documents(&self, cx: &App) -> Vec<String> {
        let mut out = vec![];
        let e = self.editor.read(cx);
        if e.is_dirty() {
            out.push(e.rel().unwrap_or_default().to_string());
        }
        out.extend(self.config_dirty(cx));
        out
    }

    fn set_env(&mut self, env: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.env == env {
            return;
        }
        self.env = env.clone();
        self.editor.update(cx, |e, cx| e.set_env(env.clone(), cx));
        if let Center::Vars(panel) = &self.center {
            let default_env = self.project.config().default_env.clone();
            panel.update(cx, |p, cx| p.set_env(env, default_env, window, cx));
        }
        self.revalidate(cx);
        cx.notify();
    }

    /// 本次会话捕获的变量 → 用例编辑器预览（「发送」时会带上）
    fn sync_session(&mut self, cx: &mut Context<Self>) {
        let vars: BTreeMap<String, String> =
            self.run.read(cx).session().vars.iter().map(|(k, v)| (k.clone(), v.to_render())).collect();
        self.editor.update(cx, |e, cx| e.set_session_vars(vars, cx));
    }

    fn clear_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run.update(cx, |r, cx| r.clear_session(cx));
        self.sync_session(cx);
        window.push_notification(Notification::info("已清空本次会话的变量与 Cookie：之后的发送从零开始"), cx);
    }

    /// 完整验证（requestbaby verify）：开始前展示全部阶段
    fn verify_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let steps = match verify::plan(self.project.root(), Some(&self.env)) {
            Ok((_, _, stages)) => stages
                .into_iter()
                .map(|s| StepView {
                    label: s.label,
                    state: if s.skip.is_some() { StepState::NotRun } else { StepState::Pending },
                    // 跳过的阶段（流程固定在其他环境）：原因显示在 stepper 说明里
                    run: s.skip.unwrap_or_default(),
                    planned: s.planned,
                    passed: 0,
                    failed: 0,
                    skipped: 0,
                })
                .collect(),
            Err(e) => {
                window.push_notification(Notification::error(format!("{e:#}")), cx);
                return;
            }
        };
        let env = self.env.clone();
        self.start_job(RunJob::Verify { env }, steps, window, cx);
    }

    /// 环境列表变化（新建 / 复制 / 删除）后同步下拉框
    fn refresh_envs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let env = pick_env(&self.project, Some(&self.env));
        let (items, ix) = (env_items(&self.project), env_index(&self.project, &env));
        self.env_select.update(cx, |s, cx| {
            s.set_items(items, window, cx);
            s.set_selected_index(ix, window, cx);
        });
        self.set_env(env, window, cx);
    }

    // ---------------------------------------------------------------- 中央编辑区

    fn config_dirty(&self, cx: &App) -> Option<String> {
        let (dirty, path) = match &self.center {
            Center::Case | Center::Envs => return None,
            Center::Collection(_, e) => (e.read(cx).is_dirty(), e.read(cx).path().clone()),
            Center::Vars(p) => return p.read(cx).dirty_label(),
            Center::Workflow(e) => return e.read(cx).dirty_label(cx),
            Center::Project(e) => (e.read(cx).is_dirty(), e.read(cx).path().clone()),
        };
        dirty.then(|| self.project.rel_of(&path))
    }

    fn center_target(&self, cx: &App) -> Option<Target> {
        match &self.center {
            Center::Case => self.editor.read(cx).rel().map(|r| Target::Case(r.to_string())),
            Center::Collection(dir, _) => Some(Target::Collection(dir.clone())),
            Center::Vars(p) => Some(Target::Vars(p.read(cx).active().cloned())),
            Center::Project(_) => Some(Target::Project),
            Center::Workflow(e) => Some(Target::Workflow(e.read(cx).key().map(String::from))),
            Center::Envs => Some(Target::Envs),
        }
    }

    /// 打开文档；会丢弃未保存修改时先确认
    fn open(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        self.adopt_env_of(&target, window, cx);
        if self.center_target(cx).as_ref() == Some(&target) {
            return;
        }
        // 变量面板内切换层：由面板自己处理未保存确认
        if let (Center::Vars(panel), Target::Vars(scope)) = (&self.center, &target) {
            let (panel, scope) = (panel.clone(), scope.clone());
            panel.update(cx, |p, cx| p.select(scope, window, cx));
            cx.notify();
            return;
        }
        let mut lost = vec![];
        lost.extend(self.config_dirty(cx));
        if let Target::Case(rel) = &target {
            let e = self.editor.read(cx);
            if e.is_dirty() && e.rel() != Some(rel.as_str()) {
                lost.push(e.rel().unwrap_or_default().to_string());
            }
        }
        if lost.is_empty() {
            self.open_now(target, window, cx);
            return;
        }
        let this = cx.entity().downgrade();
        dialogs::confirm_discard(lost, window, cx, move |window, cx| {
            let _ = this.update(cx, |w, cx| w.open_now(target.clone(), window, cx));
        });
    }

    fn open_now(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        self.adopt_env_of(&target, window, cx);
        let root = self.project.root().to_path_buf();
        match target {
            Target::Case(rel) => {
                let Some(info) = self.project.case(&rel).cloned() else { return };
                self.center = Center::Case;
                self._center_subscription = None;
                if self.editor.read(cx).rel() != Some(rel.as_str()) {
                    self.editor.update(cx, |e, cx| e.open(rel.clone(), info.path.clone(), window, cx));
                }
                // 展开祖先目录，确保打开的用例在树中可见
                let mut dir = rel.as_str();
                let mut expanded = false;
                while let Some((parent, _)) = dir.rsplit_once('/') {
                    expanded |= self.collapsed.remove(&format!("dir:{parent}"));
                    dir = parent;
                }
                if expanded {
                    self.rebuild_tree(cx);
                }
                self.select_in_tree(&format!("case:{rel}"), cx);
                self.show_response_for(&rel, window, cx);
            }
            Target::Collection(dir) => {
                let path = root.join(&dir).join("collection.json");
                let rel = self.project.rel_of(&path);
                let env = self.env.clone();
                let editor = cx.new(|cx| CollectionEditor::new(path, rel, env, window, cx));
                self._center_subscription =
                    Some(cx.subscribe_in(&editor, window, |this, _, ev, window, cx| this.on_config_event(ev, window, cx)));
                self.select_in_tree(&format!("dir:{dir}"), cx);
                self.center = Center::Collection(dir, editor);
            }
            Target::Vars(scope) => {
                let (root, env) = (self.project.root().to_path_buf(), self.env.clone());
                let default_env = self.project.config().default_env.clone();
                let panel = cx.new(|cx| VarsPanel::new(root, env, default_env, scope, false, window, cx));
                self._center_subscription =
                    Some(cx.subscribe_in(&panel, window, |this, _, ev, window, cx| this.on_vars_event(ev, window, cx)));
                self.center = Center::Vars(panel);
                self.select_in_tree("", cx);
            }
            Target::Project => {
                let path = self.project.project_file();
                let rel = self.project.rel_of(&path);
                let (env, envs) = (self.env.clone(), self.project.envs().to_vec());
                let editor = cx.new(|cx| ProjectEditor::new(path, rel, env, envs, window, cx));
                self._center_subscription =
                    Some(cx.subscribe_in(&editor, window, |this, _, ev, window, cx| this.on_config_event(ev, window, cx)));
                self.center = Center::Project(editor);
                self.select_in_tree("", cx);
            }
            Target::Workflow(key) => {
                let targets = self.run_targets();
                let editor = cx.new(|cx| WorkflowEditor::new(root, key, targets, window, cx));
                self._center_subscription =
                    Some(cx.subscribe_in(&editor, window, |this, _, ev, window, cx| this.on_workflow_event(ev, window, cx)));
                self.center = Center::Workflow(editor);
                self.select_in_tree("", cx);
            }
            Target::Envs => {
                self._center_subscription = None;
                self.center = Center::Envs;
                self.select_in_tree("", cx);
            }
        }
        cx.notify();
    }

    /// 校验问题 / 文件路径 → 对应编辑器
    fn open_path(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let rel = self.project.rel_of(path);
        let target = if rel == self.project.rel_of(&self.project.project_file()) {
            Target::Project
        } else if let Some(scope @ (VarScope::Env(_) | VarScope::Local(_))) = VarScope::of_file(&rel) {
            Target::Vars(Some(scope))
        } else if let Some(dir) = rel.strip_suffix("/collection.json") {
            Target::Collection(dir.to_string())
        } else if self.project.case(&rel).is_some() {
            Target::Case(rel)
        } else {
            return;
        };
        self.open(target, window, cx);
    }

    /// 保存中央编辑区与用例编辑器中所有未保存的修改（运行前调用，保证跑的就是看到的）
    fn save_all(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let mut ok = true;
        if self.editor.read(cx).is_dirty() {
            ok &= self.editor.update(cx, |e, cx| e.save(window, cx));
        }
        if self.config_dirty(cx).is_some() {
            ok &= self.save_center(window, cx);
        }
        ok
    }

    fn save_center(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match &self.center {
            Center::Case => self.editor.update(cx, |e, cx| e.save(window, cx)),
            Center::Collection(_, e) => e.update(cx, |e, cx| e.save(cx)),
            Center::Vars(p) => p.update(cx, |p, cx| p.save(window, cx)),
            Center::Project(e) => e.update(cx, |e, cx| e.save(cx)),
            Center::Workflow(e) => e.update(cx, |e, cx| e.save(cx)),
            Center::Envs => true,
        }
    }

    fn after_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reload_files(window, cx);
        window.push_notification(Notification::success("已保存：文件即用例，CLI 与 AI 立即可见"), cx);
    }

    fn on_config_event(&mut self, ev: &ConfigEditorEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            ConfigEditorEvent::Saved => self.after_save(window, cx),
            ConfigEditorEvent::EditVars(scope) => self.open(Target::Vars(Some(scope.clone())), window, cx),
            ConfigEditorEvent::OpenWorkflow(key) => self.open(Target::Workflow(key.clone()), window, cx),
        }
    }

    fn on_workflow_event(&mut self, ev: &WorkflowEditorEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            WorkflowEditorEvent::Saved(key) => {
                self.reload_files(window, cx);
                window.push_notification(Notification::success(format!("流程 {key} 已保存")), cx);
            }
            WorkflowEditorEvent::Deleted(key) => {
                self.center = Center::Case;
                self._center_subscription = None;
                self.reload_files(window, cx);
                if !key.is_empty() {
                    window.push_notification(Notification::success(format!("已删除流程 {key}")), cx);
                }
            }
            WorkflowEditorEvent::Duplicated(key) => {
                self.reload_files(window, cx);
                self.open_now(Target::Workflow(Some(key.clone())), window, cx);
            }
            WorkflowEditorEvent::Run { key, dry_run } => self.run_workflow(key.clone(), *dry_run, window, cx),
        }
    }

    fn on_vars_event(&mut self, ev: &VarsPanelEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            VarsPanelEvent::Saved => self.after_save(window, cx),
            VarsPanelEvent::NewEnv => self.new_env(None, window, cx),
            VarsPanelEvent::DuplicateEnv(from) => self.new_env(Some(from.clone()), window, cx),
            VarsPanelEvent::DeleteEnv(name) => self.delete_env(name.clone(), window, cx),
        }
    }

    /// 用例编辑器中点击某个变量：跳到定义它的那一层；未定义则在当前环境新增
    fn edit_var(&mut self, name: String, defined_in: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let scope = match &defined_in {
            Some(file) => match VarScope::of_file(file) {
                Some(scope) => scope,
                // 数据驱动值：定义在用例自身
                None => {
                    self.editor.update(cx, |e, cx| e.set_tab(crate::case_editor::EditorTab::Advanced, window, cx));
                    return;
                }
            },
            None => VarScope::Env(self.env.clone()),
        };
        self.open(Target::Vars(Some(scope.clone())), window, cx);
        if defined_in.is_none() {
            if let Center::Vars(panel) = &self.center {
                panel.update(cx, |p, cx| p.add_var(scope, name, window, cx));
            }
        }
    }

    /// 重新读取项目文件并刷新所有派生视图
    fn reload_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.project.reload_files();
        self.rebuild_tree(cx);
        self.refresh_envs(window, cx);
        self.revalidate(cx);
        cx.notify();
    }

    // ---------------------------------------------------------------- 新建 / 重命名 / 删除（core::scaffold）

    /// 新建用例对话框（协议 / 名称 / 地址，HTTP 可粘贴 cURL）；创建后打开编辑器
    pub(crate) fn new_case_in(&mut self, dir_rel: String, mode: NewCaseMode, window: &mut Window, cx: &mut Context<Self>) {
        let dir = self.project.root().join(&dir_rel);
        let base_url = requestbaby_core::config::load_env(self.project.root(), &self.env)
            .ok()
            .flatten()
            .and_then(|e| e.vars.get("baseUrl").map(|v| v.value.clone()));
        let this = cx.entity().downgrade();
        new_case_dialog::open(dir, dir_rel, base_url, mode, window, cx, move |path, window, cx| {
            let _ = this.update(cx, |w, cx| {
                w.reload_files(window, cx);
                let rel = w.project.rel_of(&path);
                w.open(Target::Case(rel), window, cx);
            });
        });
    }

    fn new_collection_in(&mut self, parent_rel: String, window: &mut Window, cx: &mut Context<Self>) {
        let parent = self.project.root().join(&parent_rel);
        let this = cx.entity().downgrade();
        dialogs::prompt_text(
            window,
            cx,
            "新建集合",
            format!("在 {parent_rel}/ 下创建目录与 collection.json；目录即集合"),
            "",
            "创建",
            move |name, window, cx| {
                let dir = scaffold::new_collection(&parent, &name)?;
                let _ = this.update(cx, |w, cx| {
                    w.reload_files(window, cx);
                    let rel = w.project.rel_of(&dir);
                    w.open(Target::Collection(rel), window, cx);
                });
                Ok(())
            },
        );
    }

    fn new_env(&mut self, from: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.project.root().to_path_buf();
        let this = cx.entity().downgrade();
        let (title, initial) = match &from {
            Some(f) => (format!("复制环境「{f}」"), format!("{f}-copy")),
            None => ("新建环境".to_string(), String::new()),
        };
        dialogs::prompt_text(
            window,
            cx,
            title,
            "写入 envs/<名称>.json；Secret 变量在报告与日志中一律脱敏",
            initial,
            "创建",
            move |name, window, cx| {
                requestbaby_core::envs::create(&root, &name, from.as_deref())?;
                let _ = this.update(cx, |w, cx| {
                    w.reload_files(window, cx);
                    w.open(Target::Vars(Some(VarScope::Env(name.clone()))), window, cx);
                });
                Ok(())
            },
        );
    }

    fn rename_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.dirty_inside(&path, cx) {
            window.push_notification(Notification::warning("其中有未保存的修改：请先保存或放弃后再重命名"), cx);
            return;
        }
        let initial = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let initial = initial.strip_suffix(".json").map(String::from).unwrap_or(initial);
        let this = cx.entity().downgrade();
        dialogs::prompt_text(window, cx, "重命名", "同级重命名；用例文件自动保留 .json 扩展名", initial, "重命名", move |name, window, cx| {
            let target = scaffold::rename(&path, &name)?;
            let _ = this.update(cx, |w, cx| w.after_move(&path, Some(&target), window, cx));
            Ok(())
        });
    }

    /// 复制用例 / 集合为同级新名称（core::scaffold::duplicate，与 `requestbaby case cp` / `collection cp` 同一实现）
    fn duplicate_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let stem = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let stem = stem.strip_suffix(".json").map(String::from).unwrap_or(stem);
        let what = if path.is_dir() { "集合" } else { "用例" };
        let this = cx.entity().downgrade();
        dialogs::prompt_text(window, cx, format!("复制{what}"), "复制到同级的新名称；name 字段同步改为新名称", format!("{stem}-copy"), "复制", move |name, window, cx| {
            let target = scaffold::duplicate(&path, &name)?;
            let _ = this.update(cx, |w, cx| {
                w.reload_files(window, cx);
                let rel = w.project.rel_of(&target);
                w.open(if target.is_dir() { Target::Collection(rel) } else { Target::Case(rel) }, window, cx);
            });
            Ok(())
        });
    }

    fn delete_path(&mut self, path: PathBuf, what: String, window: &mut Window, cx: &mut Context<Self>) {
        let is_dir = path.is_dir();
        let this = cx.entity().downgrade();
        let rel = self.project.rel_of(&path);
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            let path = path.clone();
            let rel_done = rel.clone();
            alert
                .title(format!("删除{what}？"))
                .description(if is_dir {
                    format!("将把目录 {rel}/ 及其中全部用例移到废纸篓，可在 Finder 中还原。")
                } else {
                    format!("将把 {rel} 移到废纸篓，可在 Finder 中还原。")
                })
                .confirm()
                .ok_text("移到废纸篓")
                .ok_variant(ButtonVariant::Danger)
                .cancel_text("取消")
                .on_ok(move |_, window, cx| {
                    match scaffold::delete(&path) {
                        Ok(how) => {
                            let msg = match how {
                                scaffold::Deleted::Trashed => format!("已移到废纸篓：{rel_done}"),
                                scaffold::Deleted::Removed => format!("系统不支持废纸篓，已永久删除：{rel_done}"),
                            };
                            window.push_notification(Notification::info(msg), cx);
                            let _ = this.update(cx, |w, cx| w.after_move(&path, None, window, cx));
                        }
                        Err(e) => window.push_notification(Notification::error(format!("{e:#}")), cx),
                    }
                    true
                })
        });
    }

    fn dirty_inside(&self, path: &Path, cx: &App) -> bool {
        let e = self.editor.read(cx);
        let case = e.is_dirty() && e.path().is_some_and(|p| p.starts_with(path));
        let config = match &self.center {
            Center::Collection(_, c) => c.read(cx).is_dirty() && c.read(cx).path().starts_with(path),
            Center::Vars(p) => p.read(cx).active().is_some_and(|s| s.file(self.project.root()).starts_with(path)) && p.read(cx).is_dirty(),
            _ => false,
        };
        case || config
    }

    /// 文件 / 目录被移动（new = Some）或删除（new = None）后修正打开的编辑器
    fn after_move(&mut self, old: &Path, new: Option<&Path>, window: &mut Window, cx: &mut Context<Self>) {
        self.project.reload_files();
        let open = self.editor.read(cx).path().cloned();
        if let Some(open) = open.filter(|p| p.starts_with(old)) {
            match new.and_then(|n| open.strip_prefix(old).ok().map(|tail| n.join(tail))) {
                Some(moved) => {
                    let rel = self.project.rel_of(&moved);
                    self.editor.update(cx, |e, cx| e.open(rel, moved, window, cx));
                }
                None => self.editor.update(cx, |e, cx| e.close(cx)),
            }
        }
        let config_path = match &self.center {
            Center::Collection(_, e) => Some(e.read(cx).path().clone()),
            Center::Vars(p) => p.read(cx).active().map(|s| s.file(self.project.root())),
            _ => None,
        };
        if config_path.is_some_and(|p| p.starts_with(old)) {
            self.center = Center::Case;
            self._center_subscription = None;
        }
        self.reload_files(window, cx);
    }

    // ---------------------------------------------------------------- 响应面板

    /// 优先本次会话的完整结果，其次共享历史中的最近结果
    fn show_response_for(&mut self, rel: &str, window: &mut Window, cx: &mut Context<Self>) {
        let session_at = self.run.read(cx).response_at(rel);
        let from_history = self.project.history().iter().find_map(|run| {
            let rows: Vec<&CaseReport> = run.results.iter().filter(|r| r.file == rel).collect();
            let row = rows.iter().find(|r| !r.passed && !r.skipped).or(rows.first())?;
            let at = run.meta.as_ref().map(|m| m.finished_at_ms).unwrap_or(0);
            Some((at, (*row).clone(), run_origin(run)))
        });
        match (session_at, from_history) {
            (Some(s_at), Some((h_at, _, _))) if s_at >= h_at => self.show_session_response(rel, window, cx),
            (Some(_), None) => self.show_session_response(rel, window, cx),
            (_, Some((_, row, origin))) => {
                self.response.update(cx, |p, cx| p.show_report(&row, &origin, window, cx));
            }
            (None, None) => self.response.update(cx, |p, cx| p.clear(window, cx)),
        }
    }

    fn show_session_response(&mut self, rel: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(r) = self.run.read(cx).response(rel).cloned() {
            self.response.update(cx, |p, cx| p.show_result(&r, "本次会话", window, cx));
        }
    }

    fn refresh_open_response(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(rel) = self.editor.read(cx).rel().map(String::from) {
            self.show_response_for(&rel, window, cx);
        }
    }

    // ---------------------------------------------------------------- 运行（core::session / core::workflow）

    /// 运行目标候选：集合目录 + 用例（相对项目根）
    fn run_targets(&self) -> Vec<String> {
        let mut out = self.project.collection_dirs();
        out.extend(self.project.cases().iter().map(|c| c.rel.clone()));
        out
    }

    fn start_job(&mut self, job: RunJob, steps: Vec<StepView>, window: &mut Window, cx: &mut Context<Self>) {
        if self.run.read(cx).is_running() {
            window.push_notification(Notification::warning("已有运行进行中"), cx);
            return;
        }
        if !self.save_all(window, cx) {
            window.push_notification(Notification::error("保存失败，未开始运行"), cx);
            return;
        }
        if !matches!(job, RunJob::Send { .. }) {
            self.bottom_tab = BottomTab::Results;
            self.selected_row = None;
        }
        let root = self.project.root().to_path_buf();
        self.run.update(cx, |r, cx| r.start(root, job, steps, cx));
        cx.notify();
    }

    fn run_paths(&mut self, paths: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let req = RunRequest { paths, env: Some(self.env.clone()), ..Default::default() };
        self.start_job(RunJob::Run(req), vec![], window, cx);
    }

    /// 重跑失败：失败的用例文件 + 它们依赖的前置用例（core::inspect::with_prerequisites，与 `run --failed` 同一规则）
    pub(crate) fn rerun_failed(&mut self, files: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let paths = requestbaby_core::inspect::with_prerequisites(self.project.root(), &files);
        let extra = paths.len().saturating_sub(files.len());
        if extra > 0 {
            window.push_notification(Notification::info(format!("重跑 {} 个失败用例，并带上 {extra} 个前置用例（它们捕获了失败用例需要的变量）", files.len())), cx);
        }
        self.run_paths(paths, window, cx);
    }

    /// 响应「字段」页：断言 / 捕获写回用例。编辑器里有未保存修改 → 追加进编辑器；否则直接写文件（core::suggest）
    fn on_response_event(&mut self, _: &Entity<ResponsePanel>, ev: &ResponsePanelEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ResponsePanelEvent::Add { file, add } = ev;
        let (file, what) = (file.clone(), add.label());
        let open_here = self.editor.read(cx).rel() == Some(file.as_str());
        if open_here && self.editor.read(cx).is_dirty() {
            self.editor.update(cx, |e, cx| e.append(add, window, cx));
            window.push_notification(Notification::info(format!("已加入编辑器（未保存，⌘S 保存）：{what}")), cx);
            return;
        }
        match requestbaby_core::suggest::append_to_case(&self.project.root().join(&file), std::slice::from_ref(add)) {
            Ok(()) => {
                if open_here {
                    self.editor.update(cx, |e, cx| {
                        e.reload_if_clean(window, cx);
                        e.set_tab(crate::case_editor::EditorTab::Expect, window, cx);
                    });
                }
                self.revalidate(cx);
                window.push_notification(Notification::success(format!("已写入 {file}：{what}")), cx);
            }
            Err(e) => window.push_notification(Notification::error(format!("{e:#}")), cx),
        }
    }

    /// ⌘P 快速打开：用例（名称 / 路径 / 方法 / URL）、流程、环境、项目配置
    fn quick_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::command::{Command, CommandItem, CommandState};
        let mut targets: Vec<Target> = vec![];
        let mut items: Vec<CommandItem> = vec![];
        for c in self.project.cases() {
            targets.push(Target::Case(c.rel.clone()));
            items.push(
                CommandItem::new()
                    .icon(IconName::File)
                    .label(format!("{}  {}  ·  {}", c.method, c.name, c.rel.trim_start_matches("collections/")))
                    .keywords([c.rel.clone(), c.url.clone()]),
            );
        }
        for (key, wf) in &self.project.config().workflows {
            targets.push(Target::Workflow(Some(key.clone())));
            let label = if wf.name.is_empty() { key.clone() } else { format!("{key} · {}", wf.name) };
            items.push(CommandItem::new().icon(IconName::Play).label(format!("流程  {label}")).keywords(["workflow".to_string()]));
        }
        targets.push(Target::Vars(Some(VarScope::Env(self.env.clone()))));
        items.push(CommandItem::new().icon(IconName::Globe).label(format!("环境变量  {}", self.env)).keywords(["env".to_string(), "vars".to_string()]));
        targets.push(Target::Envs);
        items.push(CommandItem::new().icon(IconName::Globe).label("环境管理").keywords(["env".to_string(), "环境".to_string()]));
        targets.push(Target::Vars(None));
        items.push(CommandItem::new().icon(IconName::LayoutDashboard).label("变量总览").keywords(["vars".to_string()]));
        targets.push(Target::Project);
        items.push(CommandItem::new().icon(IconName::Settings).label("项目配置 requestbaby.json").keywords(["project".to_string()]));

        let state = cx.new(|cx| CommandState::new(window, cx));
        let view = cx.entity().downgrade();
        let targets = std::rc::Rc::new(targets);
        let focus_state = state.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let (view, targets) = (view.clone(), targets.clone());
            dialog.w(px(640.)).child(
                Command::new(&state)
                    .items(items.clone())
                    .placeholder("搜索用例名 / 路径 / URL / 方法，或流程、环境…")
                    .max_h(px(440.))
                    .on_confirm(move |ix, window, cx| {
                        if let Some(t) = targets.get(ix.row).cloned() {
                            window.close_dialog(cx);
                            let _ = view.update(cx, |w, cx| w.open(t, window, cx));
                        }
                    })
                    .on_cancel(|window, cx| window.close_dialog(cx)),
            )
        });
        window.defer(cx, move |window, cx| focus_state.update(cx, |s, cx| s.focus(window, cx)));
    }

    fn run_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run_paths(vec!["collections".into()], window, cx);
    }

    fn run_workflow(&mut self, name: String, dry_run: bool, window: &mut Window, cx: &mut Context<Self>) {
        // 开始前就展示完整步骤（与 requestbaby.json 中的编排一致）
        let steps = self
            .project
            .config()
            .workflows
            .get(&name)
            .map(|wf| {
                wf.steps
                    .iter()
                    .map(|s| StepView {
                        label: workflow::step_label(s),
                        run: s.run.clone(),
                        planned: 0,
                        state: StepState::Pending,
                        passed: 0,
                        failed: 0,
                        skipped: 0,
                    })
                    .collect()
            })
            .unwrap_or_default();
        // 固定了环境的流程（如本地 fixture）始终在它自己的环境运行，不跟随右上角的环境
        let env = self.project.config().workflows.get(&name).and_then(|w| w.env.clone()).unwrap_or_else(|| self.env.clone());
        self.start_job(RunJob::Workflow { name, env, dry_run }, steps, window, cx);
    }

    // ---------------------------------------------------------------- 流程管理（core::workflow，与 CLI 同一实现）

    fn duplicate_workflow(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        let (root, this) = (self.project.root().to_path_buf(), cx.entity().downgrade());
        dialogs::prompt_text(window, cx, "复制流程", "新流程 key", format!("{key}-copy"), "复制", move |new_key, window, cx| {
            workflow::duplicate(&root, &key, &new_key)?;
            let _ = this.update(cx, |w, cx| {
                w.reload_files(window, cx);
                w.open(Target::Workflow(Some(new_key.clone())), window, cx);
            });
            Ok(())
        });
    }

    fn rename_workflow(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(&self.center, Center::Workflow(e) if e.read(cx).key() == Some(key.as_str()) && e.read(cx).dirty_label(cx).is_some()) {
            window.push_notification(Notification::warning("流程有未保存的修改：请先保存或放弃后再重命名"), cx);
            return;
        }
        let (root, this) = (self.project.root().to_path_buf(), cx.entity().downgrade());
        dialogs::prompt_text(window, cx, format!("重命名流程「{key}」"), "新的流程 key", key.clone(), "重命名", move |new_key, window, cx| {
            let new_key = new_key.trim().to_string();
            let wf = workflow::get(&root, &key)?;
            workflow::save(&root, Some(&key), &new_key, &wf)?;
            let _ = this.update(cx, |w, cx| {
                let editing = matches!(&w.center, Center::Workflow(e) if e.read(cx).key() == Some(key.as_str()));
                w.reload_files(window, cx);
                if editing {
                    w.open_now(Target::Workflow(Some(new_key.clone())), window, cx);
                }
            });
            Ok(())
        });
    }

    fn delete_workflow(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        let (root, this) = (self.project.root().to_path_buf(), cx.entity().downgrade());
        dialogs::confirm_danger(
            window,
            cx,
            format!("删除流程 {key}？"),
            "只删除 requestbaby.json 中的编排，不会删除任何用例文件。",
            "删除",
            move |window, cx| {
                let _ = this.update(cx, |w, cx| match workflow::delete(&root, &key) {
                    Ok(()) => {
                        if matches!(&w.center, Center::Workflow(e) if e.read(cx).key() == Some(key.as_str())) {
                            w.center = Center::Case;
                            w._center_subscription = None;
                        }
                        w.reload_files(window, cx);
                        window.push_notification(Notification::success(format!("已删除流程 {key}")), cx);
                    }
                    Err(e) => window.push_notification(Notification::error(format!("{e:#}")), cx),
                });
            },
        );
    }

    /// 用例树「添加到流程」：追加为最后一步
    fn add_to_workflow(&mut self, key: String, target: String, window: &mut Window, cx: &mut Context<Self>) {
        let step = requestbaby_core::config::WorkflowStep { run: target.clone(), ..Default::default() };
        match workflow::add_step(self.project.root(), &key, step, None) {
            Ok(ix) => {
                self.reload_files(window, cx);
                self.reopen_config_if_clean(&[self.project.project_file()], window, cx);
                window.push_notification(Notification::success(format!("已添加到流程 {key}：第 {} 步 {target}", ix + 1)), cx);
            }
            Err(e) => window.push_notification(Notification::error(format!("{e:#}")), cx),
        }
    }

    /// 以某个目录 / 用例为第一步新建流程
    fn new_workflow_with(&mut self, target: String, window: &mut Window, cx: &mut Context<Self>) {
        let (root, this) = (self.project.root().to_path_buf(), cx.entity().downgrade());
        let suggested = target.rsplit('/').next().unwrap_or("workflow").trim_end_matches(".json").to_string();
        dialogs::prompt_text(window, cx, "新建流程", format!("第一步：{target}"), suggested, "创建", move |key, window, cx| {
            let wf = requestbaby_core::config::Workflow {
                name: String::new(),
                env: None,
                steps: vec![requestbaby_core::config::WorkflowStep { run: target.clone(), ..Default::default() }],
            };
            workflow::save(&root, None, &key, &wf)?;
            let _ = this.update(cx, |w, cx| {
                w.reload_files(window, cx);
                w.open(Target::Workflow(Some(key.clone())), window, cx);
            });
            Ok(())
        });
    }

    fn send_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (rel, path) = {
            let e = self.editor.read(cx);
            match (e.rel(), e.path()) {
                (Some(r), Some(p)) => (r.to_string(), p.clone()),
                _ => return,
            }
        };
        let env = self.env.clone();
        self.start_job(RunJob::Send { path, rel, env }, vec![], window, cx);
    }

    /// 从树上发送：打开该用例并发送磁盘上的版本
    fn send_rel(&mut self, rel: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(info) = self.project.case(&rel).cloned() else { return };
        self.open(Target::Case(rel.clone()), window, cx);
        let env = self.env.clone();
        self.start_job(RunJob::Send { path: info.path, rel, env }, vec![], window, cx);
    }

    fn open_run_dialog(&mut self, initial: Option<RunRequest>, window: &mut Window, cx: &mut Context<Self>) {
        let init = RunDialogInit {
            root: self.project.root().to_path_buf(),
            targets: self.run_targets(),
            envs: self.project.envs().to_vec(),
            env: self.env.clone(),
            default_env: self.project.config().default_env.clone(),
            initial: initial.or_else(|| self.last_request.clone()).unwrap_or_default(),
        };
        let panel = cx.new(|cx| RunOptionsPanel::new(init, window, cx));
        let this = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let (panel_c, this) = (panel.clone(), this.clone());
            dialog
                .title("运行配置")
                .w(px(860.))
                .child(panel.clone())
                .footer(
                    h_flex()
                        .gap_2()
                        .justify_end()
                        .child(Button::new("run-cancel").outline().label("取消").on_click(|_, window, cx| window.close_dialog(cx)))
                        .child(Button::new("run-go").primary().icon(IconName::Play).label("运行").on_click(move |_, window, cx| {
                            // 变量页的修改先落盘，保证这次运行用的就是看到的值
                            if !panel_c.update(cx, |p, cx| p.save_vars(window, cx)) {
                                window.push_notification(Notification::error("变量保存失败，未开始运行"), cx);
                                return;
                            }
                            let req = panel_c.read(cx).request(cx);
                            window.close_dialog(cx);
                            let _ = this.update(cx, |w, cx| {
                                if let Some(env) = req.env.clone() {
                                    w.switch_env(env, window, cx);
                                }
                                w.last_request = Some(req.clone());
                                w.start_job(RunJob::Run(req), vec![], window, cx);
                            });
                        })),
                )
        });
    }

    /// 切换运行环境并同步标题栏下拉框
    /// 打开某个环境的变量 = 先切换到该环境：界面上只显示当前环境的变量
    fn adopt_env_of(&mut self, target: &Target, window: &mut Window, cx: &mut Context<Self>) {
        if let Target::Vars(Some(VarScope::Env(n) | VarScope::Local(n))) = target {
            if *n != self.env {
                self.switch_env(n.clone(), window, cx);
            }
        }
    }

    fn switch_env(&mut self, env: String, window: &mut Window, cx: &mut Context<Self>) {
        let ix = env_index(&self.project, &env);
        self.env_select.update(cx, |s, cx| s.set_selected_index(ix, window, cx));
        self.set_env(env, window, cx);
    }

    fn on_run_event(&mut self, _: &Entity<RunState>, ev: &RunStateEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            // 界面用例每完成一步：响应面板跟到这一步（打开的就是这个用例，或批量运行时没有手动选别的结果行）
            RunStateEvent::LiveStep { file, send } => {
                let follow = self.editor.read(cx).rel() == Some(file.as_str()) || (!*send && self.selected_row.is_none());
                if follow {
                    if let Some(live) = self.run.read(cx).live().cloned() {
                        self.response.update(cx, |p, cx| p.show_live(&live, window, cx));
                    }
                }
            }
            // 批量运行里一个用例结束：面板还停在它的「进行中」就换成最终结论
            RunStateEvent::CaseFinished { file } => {
                if self.response.read(cx).is_live_for(file) {
                    let row = self.run.read(cx).rows().iter().rev().find(|r| r.file == *file).cloned();
                    if let Some(row) = row {
                        self.response.update(cx, |p, cx| p.show_report(&row, "本次运行", window, cx));
                    }
                }
            }
            RunStateEvent::CaseDone { file } => {
                if self.editor.read(cx).rel() == Some(file.as_str()) || self.response.read(cx).is_live_for(file) {
                    self.show_session_response(&file.clone(), window, cx);
                }
            }
            RunStateEvent::Finished { send, workflow, verify, blocked, failed, total, dry_run } => {
                self.sync_session(cx);
                if *send {
                    return;
                }
                if !*dry_run {
                    self.reload_history(cx);
                }
                let what = match (verify, workflow) {
                    (true, _) => "完整验证".to_string(),
                    (_, Some(w)) => format!("流程「{w}」"),
                    _ => "运行".into(),
                };
                let summary = self.run.read(cx).report().map(|r| r.failure_text()).unwrap_or_default();
                let note = if let Some(why) = blocked {
                    // 不自动消失：原因可能很长，点「复制详情」拿到完整文本
                    with_copy(Notification::error(format!("{what}无法完成：{}", why.lines().next().unwrap_or_default())), summary)
                } else if *dry_run {
                    Notification::info(format!("{what} dry-run 完成：{total} 个用例编译通过，未发送请求"))
                } else if *failed == 0 {
                    Notification::success(format!("{what}：{total} 个用例全部通过，已写入共享历史"))
                } else {
                    with_copy(Notification::warning(format!("{what}：{failed}/{total} 个用例失败 —— 在结果中查看现场")), summary).autohide(true)
                };
                window.push_notification(note, cx);
            }
            RunStateEvent::Prerequisite(msg) => {
                // 通知里只留文件名：完整路径太长（CLI 仍输出完整路径）
                let short = match msg.split_once('：') {
                    Some((head, tail)) => match head.rsplit_once('/') {
                        Some((_, file)) => format!("先运行前置用例「{}」：{tail}", file.trim_end_matches(".json")),
                        None => msg.clone(),
                    },
                    None => msg.clone(),
                };
                window.push_notification(Notification::info(short), cx)
            }
            RunStateEvent::Error(msg) => window.push_notification(with_copy(Notification::error(msg.clone()), msg.clone()), cx),
        }
        cx.notify();
    }

    fn reload_history(&mut self, cx: &mut Context<Self>) -> Vec<RunReport> {
        self.project.reload_history();
        self.statuses = self.project.latest_status();
        let mut fresh = vec![];
        for r in self.project.history() {
            if let Some(m) = &r.meta {
                if self.seen_runs.insert(m.id.clone()) {
                    fresh.push(r.clone());
                }
            }
        }
        cx.notify();
        fresh
    }

    fn copy_project_path(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.project.root().display().to_string();
        cx.write_to_clipboard(ClipboardItem::new_string(path.clone()));
        style::push_brief(window, cx, Notification::success(format!("已复制项目路径：{path}")));
    }

    /// 导出结果：先选范围（可勾选多个集合）与格式，并看清报告里有什么（`requestbaby export` 同一实现）
    fn open_export_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.project.root().to_path_buf();
        let env = self.env.clone();
        let runs: Vec<&RunReport> = self.project.history().iter().collect();
        let mut items = vec![];
        if let Some(latest) = requestbaby_core::history::merge_latest(&runs, &env, &[]) {
            for (dir, n) in export::collections(&latest) {
                if let Some(r) = requestbaby_core::history::merge_latest(&runs, &env, std::slice::from_ref(&dir)) {
                    items.push(CollectionItem::new(dir, n, &r, &root));
                }
            }
        }
        let run = self.run.read(cx);
        let current = run.report().filter(|_| !run.is_running()).and_then(|report| {
            let m = report.meta.as_ref().filter(|m| !m.dry_run)?;
            let title = format!("当前显示的这一次运行（{}，{}）", m.command, relative_time(m.started_at_ms));
            Some(RunChoice::new(title, report.clone(), &root, m.id.clone()))
        });
        if items.is_empty() && current.is_none() {
            window.push_notification(Notification::warning("还没有可导出的运行结果：先运行一次（⌘R 运行全部）"), cx);
            return;
        }
        let panel = cx.new(|_| ExportPanel::new(env.clone(), items, current));
        let this = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let (panel_c, this, env) = (panel.clone(), this.clone(), env.clone());
            dialog
                .title("导出结果")
                .w(px(680.))
                .child(panel.clone())
                .footer(
                    h_flex()
                        .gap_2()
                        .justify_end()
                        .child(Button::new("export-cancel").outline().label("取消").on_click(|_, window, cx| window.close_dialog(cx)))
                        .child(Button::new("export-go").primary().icon(IconName::ExternalLink).label("导出…").on_click(move |_, window, cx| {
                            let (selection, format) = match panel_c.read(cx).selection() {
                                Ok(s) => s,
                                Err(why) => {
                                    window.push_notification(Notification::warning(why), cx);
                                    return;
                                }
                            };
                            window.close_dialog(cx);
                            let env = env.clone();
                            let _ = this.update(cx, |w, cx| {
                                let report = match selection {
                                    Selection::Run(r) => Some(*r),
                                    Selection::Latest(dirs) => {
                                        let runs: Vec<&RunReport> = w.project.history().iter().collect();
                                        requestbaby_core::history::merge_latest(&runs, &env, &dirs)
                                    }
                                };
                                match report {
                                    Some(r) => w.export_to_file(r, format, window, cx),
                                    None => window.push_notification(Notification::warning("选中的集合没有可导出的结果"), cx),
                                }
                            });
                        })),
                )
        });
    }

    /// 选保存位置 → 生成报告（HTML 内嵌截图，放后台）→ 写文件 → 在 Finder 中选中
    fn export_to_file(&mut self, report: RunReport, format: export::Format, window: &mut Window, cx: &mut Context<Self>) {
        let name = export::file_name(self.project.root(), &report, format);
        let rx = cx.prompt_for_new_path(&crate::response::downloads_dir(), Some(&name));
        let root = self.project.root().to_path_buf();
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(path))) = rx.await else { return };
            let overview = export::Overview::of(&report, &root, &export::collections_of_command(&report));
            let (root, out) = (root.clone(), path.clone());
            let written = cx
                .background_executor()
                .spawn(async move { requestbaby_core::fsx::write(&out, export::render(&report, &root, format)).map(|_| out) })
                .await;
            let _ = cx.update(|window, cx| match written {
                Ok(path) => {
                    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    cx.reveal_path(&path);
                    let text = format!("已导出 {}（{}，{}）", path.display(), overview.headline(), human_size(size));
                    style::push_brief(window, cx, Notification::success(text));
                }
                Err(e) => window.push_notification(Notification::error(format!("导出失败：{e:#}")), cx),
            });
        })
        .detach();
    }

    // ---------------------------------------------------------------- 校验 / 格式化 / 自检

    fn revalidate(&mut self, cx: &mut Context<Self>) {
        let (root, env) = (self.project.root().to_path_buf(), self.env.clone());
        self.validation.running = true;
        // 与 CLI `validate` / verify 同一范围：collections + 本环境会运行的流程引用的目录
        let task = cx.background_spawn(async move { validate::validate_paths(&validate::project_scope(&root, &env), &env) });
        self._validate_task = Some(cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |w, cx| {
                let v = &mut w.validation;
                v.running = false;
                v.done = true;
                match res {
                    Ok(issues) => {
                        v.issues = issues;
                        v.error = None;
                    }
                    Err(e) => v.error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn format_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.dirty_documents(cx).is_empty() {
            window.push_notification(Notification::warning("有未保存的修改：请先保存后再格式化"), cx);
            return;
        }
        let dir = self.project.collections_dir();
        let preview = match fmt::format_paths(std::slice::from_ref(&dir), true, true) {
            Ok(r) => r,
            Err(e) => {
                window.push_notification(Notification::error(format!("{e:#}")), cx);
                return;
            }
        };
        let n = preview.case_changes();
        if n == 0 {
            window.push_notification(Notification::success("所有用例已是规范格式，也没有可自动改写的写法"), cx);
            return;
        }
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (this, dir) = (this.clone(), dir.clone());
            alert
                .title(format!("格式化 {n} 个用例？"))
                .description(
                    "按写法建议改写（JS 断言 → 断言表格、post 取值 → 捕获、写死的域名 → {{baseUrl}} 等，语义不变）；字段按可读顺序排列、清理默认值；目录内所有用例相同的请求头上提到 collection.json。",
                )
                .confirm()
                .ok_text("格式化")
                .cancel_text("取消")
                .on_ok(move |_, window, cx| {
                    match fmt::format_paths(std::slice::from_ref(&dir), false, true) {
                        Ok(r) => {
                            let hoisted = r.changes.len() - r.case_changes();
                            let mut text = format!("已格式化 {} 个用例", r.case_changes());
                            if hoisted > 0 {
                                text.push_str(&format!("，{hoisted} 个目录的公共请求头已上提"));
                            }
                            window.push_notification(Notification::success(text), cx);
                            let _ = this.update(cx, |w, cx| {
                                w.editor.update(cx, |e, cx| e.reload_if_clean(window, cx));
                                w.reload_files(window, cx);
                            });
                        }
                        Err(e) => window.push_notification(Notification::error(format!("{e:#}")), cx),
                    }
                    true
                })
        });
    }

    fn run_doctor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        dialogs::show_doctor(doctor::run(self.project.root(), Some(&self.env)), window, cx);
    }

    // ---------------------------------------------------------------- 文件监听（CLI / AI ↔ Desktop）

    fn start_watching(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (tx, mut rx) = mpsc::unbounded::<PathBuf>();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                for p in ev.paths {
                    let _ = tx.unbounded_send(p);
                }
            }
        });
        let Ok(mut watcher) = watcher else { return };
        if watcher.watch(self.project.root(), notify::RecursiveMode::Recursive).is_err() {
            return;
        }
        self._watcher = Some(watcher);
        self._watch_task = Some(cx.spawn_in(window, async move |this, cx| {
            while let Some(first) = rx.next().await {
                // 合并一阵突发写入（AI 批量生成 / fmt）
                cx.background_executor().timer(Duration::from_millis(250)).await;
                let mut paths = vec![first];
                while let Ok(p) = rx.try_recv() {
                    paths.push(p);
                }
                let alive = this.update_in(cx, |w, window, cx| w.on_files_changed(paths, window, cx)).is_ok();
                if !alive {
                    break;
                }
            }
        }));
    }

    fn on_files_changed(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let state_dir = requestbaby_core::history::state_dir(self.project.root());
        // 只有运行历史算「历史变化」：.requestbaby/ 下的其他文件不触发重载
        let runs_dir = requestbaby_core::history::runs_dir(self.project.root());
        let history_changed = paths.iter().any(|p| p.starts_with(&runs_dir));
        let files_changed = paths.iter().any(|p| !p.starts_with(&state_dir) && (p.extension().is_none_or(|x| x == "json")));
        if files_changed {
            self.reload_files(window, cx);
            let open = self.editor.read(cx).path().cloned();
            if let Some(open) = open {
                if !open.exists() {
                    self.editor.update(cx, |e, cx| e.close(cx));
                    window.push_notification(Notification::warning("打开的用例已在外部被删除或移动"), cx);
                } else if paths.contains(&open) {
                    if self.editor.update(cx, |e, cx| e.reload_if_clean(window, cx)) {
                        window.push_notification(Notification::info("用例已在外部被修改（AI / 编辑器），已同步最新内容"), cx);
                    } else if self.editor.read(cx).is_dirty() {
                        window.push_notification(Notification::warning("用例在外部被修改，但你有未保存的编辑；保存将覆盖外部修改"), cx);
                    }
                }
            }
            self.reopen_config_if_clean(&paths, window, cx);
            // 变量定义可能变了：刷新用例编辑器里的解析预览
            self.editor.update(cx, |e, cx| e.refresh_vars(cx));
        }
        if history_changed && !self.run.read(cx).is_running() {
            let fresh = self.reload_history(cx);
            let external: Vec<RunReport> =
                fresh.into_iter().filter(|r| r.meta.as_ref().is_some_and(|m| m.source != "desktop")).collect();
            if let Some(latest) = external.first().cloned() {
                self.run.update(cx, |r, cx| r.show_report(&latest, cx));
                self.selected_row = None;
                self.refresh_open_response(window, cx);
                self.bottom_tab = BottomTab::Results;
                let m = latest.meta.clone().unwrap_or_default();
                let text = format!("{} 刚运行：{}/{} 通过 · {}", m.source.to_uppercase(), latest.passed, latest.total, m.command);
                let n = if latest.failed > 0 { Notification::warning(text) } else { Notification::info(text) };
                window.push_notification(n, cx);
            }
        }
        cx.notify();
    }

    /// 打开的配置文件在外部被改：未编辑则重新载入，被删则关闭
    fn reopen_config_if_clean(&mut self, changed: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        if let Center::Workflow(editor) = &self.center {
            let (editor, targets) = (editor.clone(), self.run_targets());
            editor.update(cx, |e, cx| {
                e.set_targets(targets);
                e.reload_if_clean(window, cx)
            });
            return;
        }
        if let Center::Vars(panel) = &self.center {
            // 任一变量文件变化都会影响总览与覆盖提示
            let root = self.project.root().to_path_buf();
            if changed.iter().any(|p| VarScope::of_file(&self.project.rel_of(p)).is_some() || !p.starts_with(&root)) {
                panel.update(cx, |p, cx| p.reload_if_clean(window, cx));
            }
            return;
        }
        let Some(target) = self.center_target(cx).filter(|_| !matches!(self.center, Center::Case)) else { return };
        let path = match &self.center {
            Center::Collection(_, e) => e.read(cx).path().clone(),
            Center::Project(e) => e.read(cx).path().clone(),
            Center::Case | Center::Vars(_) | Center::Workflow(_) | Center::Envs => return,
        };
        if !changed.contains(&path) || self.config_dirty(cx).is_some() {
            return;
        }
        if !path.exists() && !matches!(target, Target::Collection(_)) {
            self.center = Center::Case;
            self._center_subscription = None;
            return;
        }
        self.center = Center::Case;
        self.open_now(target, window, cx);
    }

    // ---------------------------------------------------------------- 渲染

    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let running = self.run.read(cx).is_running();
        let view = cx.entity().downgrade();
        let root = self.project.root().to_path_buf();
        TitleBar::new()
            .bg(style::canvas(cx))
            .child(
                h_flex()
                    .gap_2()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(Icon::new(IconName::Bot).small().text_color(cx.theme().primary))
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("requestbaby"))
                    .child(div().text_sm().text_color(cx.theme().muted_foreground).child("/"))
                    .child(
                        Button::new("project-menu")
                            .ghost()
                            .small()
                            .label(self.project.name())
                            .icon(IconName::ChevronDown)
                            .dropdown_menu(move |menu, _, _| {
                                let root = root.clone();
                                menu.item(menu_item(&view, "打开项目…", IconName::FolderOpen, |_, _, cx| cx.emit(ShellRequest::OpenProject)))
                                    .item(menu_item(&view, "新建项目…", IconName::Plus, |_, _, cx| cx.emit(ShellRequest::NewProject)))
                                    .item(menu_item(&view, "从 OpenAPI / Postman 导入…", IconName::Inbox, |_, _, cx| {
                                        cx.emit(ShellRequest::ImportSpec)
                                    }))
                                    .separator()
                                    .item(menu_item(&view, "项目设置", IconName::Settings2, |w, window, cx| w.open(Target::Project, window, cx)))
                                    .item(menu_item(&view, "复制项目路径", IconName::Copy, |w, window, cx| w.copy_project_path(window, cx)))
                                    .item(PopupMenuItem::new("在 Finder 中显示").icon(IconName::Folder).on_click(move |_, _, cx| cx.reveal_path(&root)))
                                    .separator()
                                    .item(menu_item(&view, "关闭项目", IconName::Close, |_, _, cx| cx.emit(ShellRequest::CloseProject)))
                            }),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .pr_2()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(div().text_xs().text_color(cx.theme().muted_foreground).child("环境"))
                    .child(div().w(px(120.)).child(Select::new(&self.env_select).small()))
                    .map(|this| {
                        if running {
                            this.child(
                                Button::new("stop-run")
                                    .small()
                                    .danger()
                                    .icon(IconName::Square)
                                    .label("停止")
                                    .on_click(cx.listener(|this, _, _, cx| this.run.update(cx, |r, cx| r.stop(cx)))),
                            )
                        } else {
                            this.child(
                                Button::new("run-all")
                                    .small()
                                    .ghost()
                                    .icon(IconName::Play)
                                    .label("运行全部")
                                    .tooltip("requestbaby run collections（⌘R）")
                                    .on_click(cx.listener(|this, _, window, cx| this.run_all(window, cx))),
                            )
                            .child(
                                Button::new("verify")
                                    .small()
                                    .primary()
                                    .rounded(style::PILL)
                                    .icon(IconName::CircleCheck)
                                    .label("完整验证")
                                    .tooltip("先检查配置，再跑全部用例和流程，给出最终结论（requestbaby verify，⇧⌘V）")
                                    .on_click(cx.listener(|this, _, window, cx| this.verify_all(window, cx))),
                            )
                        }
                    })
                    .child(self.render_more_menu(cx)),
            )
    }

    /// 标题栏右侧的「⋯」：不常用的操作都收在这里（与菜单栏同源，菜单栏里也能找到）
    fn render_more_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        let proxy = self.proxy_mode();
        let style_brief = {
            let b = validate::ai_brief(&self.validation.issues, &self.env);
            (!b.is_empty()).then_some(b)
        };
        Button::new("more-menu").small().ghost().icon(IconName::Ellipsis).tooltip("更多").dropdown_menu(move |menu, _, _| {
            let menu = menu
                .item(menu_item(&view, "运行配置…（选范围、标签、并发）", IconName::Settings2, |w, window, cx| w.open_run_dialog(None, window, cx)))
                .item(menu_item(&view, "导出结果…", IconName::ExternalLink, |w, window, cx| w.open_export_dialog(window, cx)))
                .separator()
                .item(menu_item(&view, "环境管理…", IconName::Globe, |w, window, cx| w.open(Target::Envs, window, cx)))
                .item(menu_item(&view, "变量…", IconName::Settings2, |w, window, cx| {
                    let scope = VarScope::Env(w.env.clone());
                    w.open(Target::Vars(Some(scope)), window, cx)
                }));
            let menu = Workspace::proxy_menu_items(menu, &view, proxy)
                .separator()
                .item(menu_item(&view, "检查项目配置", IconName::CircleCheck, |w, _, cx| {
                    w.bottom_tab = BottomTab::Issues;
                    w.revalidate(cx);
                }))
                .item(menu_item(&view, "整理用例格式", IconName::FileText, |w, window, cx| w.format_project(window, cx)))
                .item(menu_item(&view, "运行环境自检", IconName::Settings, |w, window, cx| w.run_doctor(window, cx)))
                .separator()
                .item(menu_item(&view, "复制用例写法速查（粘贴给 AI 对话）", IconName::Bot, |_, window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(guide::quickstart()));
                    window.push_notification(
                        Notification::success("已复制用例写法速查卡：粘贴到 ChatGPT / Claude 等对话里，AI 就会按 requestbaby 的格式写用例"),
                        cx,
                    );
                }));
            let menu = match style_brief.clone() {
                Some(brief) => menu.item(PopupMenuItem::new("复制写法修改指令（给 AI）").icon(IconName::Bot).on_click(move |_, window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(brief.clone()));
                    style::push_brief(window, cx, Notification::success("已复制修改指令：粘贴给 AI 即可"));
                })),
                None => menu,
            };
            menu.separator()
                .item(Workspace::check_update_item(&view))
                .item(Workspace::auto_update_item(&view))
                .separator()
                .item(PopupMenuItem::new("切换深色 / 浅色").icon(IconName::Moon).on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleTheme), cx)))
        })
    }

    fn render_center(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let border = cx.theme().border;
        match &self.center {
            Center::Case => h_resizable("editor-split")
                .child(resizable_panel().child(self.editor.clone()))
                .child(
                    resizable_panel()
                        .size(px(480.))
                        .size_range(px(320.)..px(900.))
                        .child(div().size_full().border_l_1().border_color(border).child(self.response.clone())),
                )
                .into_any_element(),
            Center::Collection(_, e) => div().size_full().child(e.clone()).into_any_element(),
            Center::Vars(p) => div().size_full().child(p.clone()).into_any_element(),
            Center::Project(e) => div().size_full().child(e.clone()).into_any_element(),
            Center::Workflow(e) => div().size_full().child(e.clone()).into_any_element(),
            Center::Envs => self.render_env_manager(cx),
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("workspace")
            .key_context("Workspace")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &SaveCase, window, cx| {
                this.save_center(window, cx);
            }))
            .on_action(cx.listener(|this, _: &SendCase, window, cx| this.send_current(window, cx)))
            .on_action(cx.listener(|this, _: &QuickOpen, window, cx| this.quick_open(window, cx)))
            .on_action(cx.listener(|this, _: &FindInTree, window, cx| {
                let focus = this.tree_filter.read(cx).focus_handle(cx);
                focus.focus(window, cx);
            }))
            .on_action(cx.listener(|this, _: &RunAll, window, cx| this.run_all(window, cx)))
            .on_action(cx.listener(|this, _: &CopyProjectPath, window, cx| this.copy_project_path(window, cx)))
            .on_action(cx.listener(|this, _: &ExportResults, window, cx| this.open_export_dialog(window, cx)))
            .on_action(cx.listener(|this, _: &ManageEnvs, window, cx| this.open(Target::Envs, window, cx)))
            .on_action(cx.listener(|this, _: &Verify, window, cx| this.verify_all(window, cx)))
            .on_action(cx.listener(|this, _: &RunWithOptions, window, cx| this.open_run_dialog(None, window, cx)))
            .on_action(cx.listener(|this, _: &StopRun, _, cx| this.run.update(cx, |r, cx| r.stop(cx))))
            .size_full()
            .bg(style::canvas(cx))
            .text_color(cx.theme().foreground)
            .child(self.render_title_bar(cx))
            // 侧栏直接放在画布上；编辑区与结果是两张浮起的卡片，分隔条透明（悬停才出现拖动提示）
            .child(
                div().flex_1().min_h_0().pr(px(style::GAP)).child(
                    h_resizable("main-split")
                        .with_handle_appearance(style::gap_handle())
                        .child(
                            resizable_panel()
                                .size(px(280.))
                                .size_range(px(220.)..px(460.))
                                .child(self.render_sidebar(cx)),
                        )
                        .child(
                            v_resizable("work-split")
                                .with_handle_appearance(style::gap_handle())
                                .child(resizable_panel().child(
                                    div().size_full().pb(px(style::GAP / 2.)).child(style::card(cx).size_full().child(self.render_center(cx))),
                                ))
                                .child(
                                    resizable_panel().size(px(280.)).size_range(px(140.)..px(640.)).child(
                                        div()
                                            .size_full()
                                            .pt(px(style::GAP / 2.))
                                            .pb(px(style::GAP))
                                            .child(style::card(cx).size_full().child(self.render_bottom(cx))),
                                    ),
                                ),
                        ),
                ),
            )
            .child(self.render_status_bar(cx))
    }
}

/// 选择环境：保持当前（若仍存在）→ 项目默认 → 第一个
fn pick_env(project: &Project, current: Option<&str>) -> String {
    let envs = project.envs();
    let default = &project.config().default_env;
    current
        .filter(|c| envs.iter().any(|e| e == c))
        .map(String::from)
        .or_else(|| envs.contains(default).then(|| default.clone()))
        .or_else(|| envs.first().cloned())
        .unwrap_or_else(|| default.clone())
}

fn env_items(project: &Project) -> Vec<SharedString> {
    project.envs().iter().map(|e| SharedString::from(e.clone())).collect()
}

fn env_index(project: &Project, env: &str) -> Option<IndexPath> {
    project.envs().iter().position(|e| e == env).map(IndexPath::new)
}

fn run_origin(r: &RunReport) -> String {
    let m = r.meta.clone().unwrap_or_default();
    format!("{} · {}", m.source.to_uppercase(), relative_time(m.started_at_ms))
}

fn human_size(bytes: u64) -> String {
    match bytes {
        0..=1023 => format!("{bytes} B"),
        1024..=1_048_575 => format!("{:.0} KB", bytes as f64 / 1024.),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.),
    }
}

fn relative_time(ms: u64) -> String {
    let now = requestbaby_core::history::now_ms();
    let secs = now.saturating_sub(ms) / 1000;
    match secs {
        0..=59 => "刚刚".into(),
        60..=3599 => format!("{} 分钟前", secs / 60),
        3600..=86_399 => format!("{} 小时前", secs / 3600),
        _ => format!("{} 天前", secs / 86_400),
    }
}

/// 通知附「复制详情」：错误信息可完整复制（通知本身的文字不能选中）
fn with_copy(note: Notification, text: String) -> Notification {
    if text.is_empty() {
        return note;
    }
    note.action(move |_, _, _| {
        let text = text.clone();
        Button::new("copy-detail").xsmall().outline().icon(IconName::Copy).label("复制详情").on_click(move |_, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        })
    })
}

