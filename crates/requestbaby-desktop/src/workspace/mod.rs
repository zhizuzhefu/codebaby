//! 工作区：标题栏 / 侧栏（集合树·工作流·环境·项目）/ 中央编辑区 / 底部（结果·日志·历史·校验）/ 状态栏。
//!
//! 所有能力都调用 requestbaby-core，与 CLI 同一实现：运行走 `session` / `workflow`，
//! 新建 / 重命名 / 删除走 `scaffold`，校验 / 格式化 / 自检走 `validate` / `fmt` / `doctor`。
//! 文件监听让两端共享同一份事实：AI 改了文件 → 树与编辑器刷新；终端跑了一次 → 结果立刻出现。
mod bottom;
mod sidebar;

use crate::case_editor::{CaseEditor, CaseEditorEvent};
use crate::config_editors::{CollectionEditor, ConfigEditorEvent, EnvEditor, ProjectEditor};
use crate::dialogs;
use sidebar::menu_item;
use crate::project::{CaseStatus, Project};
use crate::response::ResponsePanel;
use crate::run::{RunJob, RunState, RunStateEvent, StepState, StepView};
use crate::run_dialog::RunOptionsPanel;
use crate::shell::ShellRequest;
use crate::{RunAll, RunWithOptions, SaveCase, SendCase, StopRun, ToggleTheme};
use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, IndexPath, Sizable as _, TitleBar, WindowExt as _, h_flex,
    button::{Button, ButtonVariant, ButtonVariants as _},
    menu::{DropdownMenu as _, PopupMenuItem},
    notification::Notification,
    resizable::{h_resizable, resizable_panel, v_resizable},
    select::{Select, SelectEvent, SelectState},
    tree::{TreeEvent, TreeState},
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, InteractiveElement as _,
    IntoElement, MouseButton, ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, Window, div,
    prelude::FluentBuilder as _, px,
};
use notify::Watcher as _;
use requestbaby_core::report::{CaseReport, RunReport};
use requestbaby_core::session::RunRequest;
use requestbaby_core::validate::ValidationIssue;
use requestbaby_core::{doctor, fmt, report, scaffold, validate, workflow};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BottomTab {
    Results,
    Logs,
    History,
    Validate,
}

impl BottomTab {
    const ALL: [BottomTab; 4] = [BottomTab::Results, BottomTab::Logs, BottomTab::History, BottomTab::Validate];
}

/// 中央编辑区可打开的文档
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    /// 用例（相对项目根）
    Case(String),
    /// 集合目录（相对项目根）的 collection.json
    Collection(String),
    /// envs/<name>.json
    Env(String),
    /// requestbaby.json
    Project,
}

/// 中央编辑区当前内容。用例编辑器常驻（切去配置再回来不丢编辑），配置编辑器按需创建
enum Center {
    Case,
    Collection(String, Entity<CollectionEditor>),
    Env(String, Entity<EnvEditor>),
    Project(Entity<ProjectEditor>),
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
    center: Center,
    editor: Entity<CaseEditor>,
    response: Entity<ResponsePanel>,
    run: Entity<RunState>,
    bottom_tab: BottomTab,
    selected_row: Option<usize>,
    statuses: HashMap<String, CaseStatus>,
    seen_runs: HashSet<String>,
    validation: Validation,
    /// 运行配置对话框上次的参数
    last_request: Option<RunRequest>,
    focus_handle: FocusHandle,
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
        let editor = cx.new(|cx| CaseEditor::new(window, cx));
        editor.update(cx, |e, _| e.set_env(env.clone()));
        let response = cx.new(|cx| ResponsePanel::new(window, cx));
        let run = cx.new(|_| RunState::new());

        let subs = vec![
            cx.subscribe_in(&env_select, window, |this, _, ev: &SelectEvent<Vec<SharedString>>, _, cx| {
                if let SelectEvent::Confirm(Some(env)) = ev {
                    this.set_env(env.to_string(), cx);
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
            }),
            cx.subscribe_in(&run, window, Self::on_run_event),
        ];

        let seen_runs = project.history().iter().filter_map(|r| r.meta.as_ref().map(|m| m.id.clone())).collect();
        let mut this = Self {
            project,
            env,
            env_select,
            tree,
            collapsed: HashSet::new(),
            center: Center::Case,
            editor,
            response,
            run,
            bottom_tab: BottomTab::Results,
            selected_row: None,
            statuses: HashMap::new(),
            seen_runs,
            validation: Validation::default(),
            last_request: None,
            focus_handle: cx.focus_handle(),
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

    fn set_env(&mut self, env: String, cx: &mut Context<Self>) {
        if self.env == env {
            return;
        }
        self.env = env.clone();
        self.editor.update(cx, |e, _| e.set_env(env));
        self.revalidate(cx);
        cx.notify();
    }

    /// 环境列表变化（新建 / 复制 / 删除）后同步下拉框
    fn refresh_envs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let env = pick_env(&self.project, Some(&self.env));
        let (items, ix) = (env_items(&self.project), env_index(&self.project, &env));
        self.env_select.update(cx, |s, cx| {
            s.set_items(items, window, cx);
            s.set_selected_index(ix, window, cx);
        });
        self.set_env(env, cx);
    }

    // ---------------------------------------------------------------- 中央编辑区

    fn config_dirty(&self, cx: &App) -> Option<String> {
        let (dirty, path) = match &self.center {
            Center::Case => return None,
            Center::Collection(_, e) => (e.read(cx).is_dirty(), e.read(cx).path().clone()),
            Center::Env(_, e) => (e.read(cx).is_dirty(), e.read(cx).path().clone()),
            Center::Project(e) => (e.read(cx).is_dirty(), e.read(cx).path().clone()),
        };
        dirty.then(|| self.project.rel_of(&path))
    }

    fn center_target(&self, cx: &App) -> Option<Target> {
        match &self.center {
            Center::Case => self.editor.read(cx).rel().map(|r| Target::Case(r.to_string())),
            Center::Collection(dir, _) => Some(Target::Collection(dir.clone())),
            Center::Env(name, _) => Some(Target::Env(name.clone())),
            Center::Project(_) => Some(Target::Project),
        }
    }

    /// 打开文档；会丢弃未保存修改时先确认
    fn open(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        if self.center_target(cx).as_ref() == Some(&target) {
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
        confirm_discard(lost, window, cx, move |window, cx| {
            let _ = this.update(cx, |w, cx| w.open_now(target.clone(), window, cx));
        });
    }

    fn open_now(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
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
            Target::Env(name) => {
                let path = self.project.env_path(&name);
                let rel = self.project.rel_of(&path);
                let is_default = self.project.config().default_env == name;
                let editor = cx.new(|cx| EnvEditor::new(path, rel, name.clone(), is_default, window, cx));
                self._center_subscription =
                    Some(cx.subscribe_in(&editor, window, |this, _, ev, window, cx| this.on_config_event(ev, window, cx)));
                self.center = Center::Env(name, editor);
                self.select_in_tree("", cx);
            }
            Target::Project => {
                let path = self.project.project_file();
                let rel = self.project.rel_of(&path);
                let (env, envs, targets) = (self.env.clone(), self.project.envs().to_vec(), self.run_targets());
                let editor = cx.new(|cx| ProjectEditor::new(path, rel, env, envs, targets, window, cx));
                self._center_subscription =
                    Some(cx.subscribe_in(&editor, window, |this, _, ev, window, cx| this.on_config_event(ev, window, cx)));
                self.center = Center::Project(editor);
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
        } else if let Some(name) = rel.strip_prefix("envs/").and_then(|n| n.strip_suffix(".json")) {
            Target::Env(name.to_string())
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
            Center::Env(_, e) => e.update(cx, |e, cx| e.save(cx)),
            Center::Project(e) => e.update(cx, |e, cx| e.save(cx)),
        }
    }

    fn after_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reload_files(window, cx);
        window.push_notification(Notification::success("已保存：文件即用例，CLI 与 AI 立即可见"), cx);
    }

    fn on_config_event(&mut self, ev: &ConfigEditorEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            ConfigEditorEvent::Saved => self.after_save(window, cx),
            ConfigEditorEvent::DuplicateEnv(from) => self.new_env(Some(from.clone()), window, cx),
            ConfigEditorEvent::DeleteEnv(name) => {
                let path = self.project.env_path(name);
                self.delete_path(path, format!("环境「{name}」"), window, cx);
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

    fn new_case_in(&mut self, dir_rel: String, window: &mut Window, cx: &mut Context<Self>) {
        let dir = self.project.root().join(&dir_rel);
        let this = cx.entity().downgrade();
        dialogs::prompt_text(
            window,
            cx,
            "新建用例",
            format!("在 {dir_rel}/ 下创建 NN-<名称>.json；等价命令 requestbaby new case {dir_rel} <名称>"),
            "",
            "创建",
            move |name, window, cx| {
                let path = scaffold::new_case(&dir, &name, "GET", "")?;
                let _ = this.update(cx, |w, cx| {
                    w.reload_files(window, cx);
                    let rel = w.project.rel_of(&path);
                    w.open(Target::Case(rel), window, cx);
                });
                Ok(())
            },
        );
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
                scaffold::new_env(&root, &name, from.as_deref())?;
                let _ = this.update(cx, |w, cx| {
                    w.reload_files(window, cx);
                    w.open(Target::Env(name.clone()), window, cx);
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

    fn delete_path(&mut self, path: PathBuf, what: String, window: &mut Window, cx: &mut Context<Self>) {
        let is_dir = path.is_dir();
        let this = cx.entity().downgrade();
        let rel = self.project.rel_of(&path);
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            let path = path.clone();
            alert
                .title(format!("删除{what}？"))
                .description(if is_dir {
                    format!("将删除目录 {rel}/ 及其中全部用例。文件删除后只能从版本控制恢复。")
                } else {
                    format!("将删除 {rel}。文件删除后只能从版本控制恢复。")
                })
                .confirm()
                .ok_text("删除")
                .ok_variant(ButtonVariant::Danger)
                .cancel_text("取消")
                .on_ok(move |_, window, cx| {
                    match scaffold::delete(&path) {
                        Ok(()) => {
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
            Center::Env(_, c) => c.read(cx).is_dirty() && c.read(cx).path().starts_with(path),
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
            Center::Env(_, e) => Some(e.read(cx).path().clone()),
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
        let env = self.env.clone();
        self.start_job(RunJob::Workflow { name, env, dry_run }, steps, window, cx);
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
        let initial = initial.or_else(|| self.last_request.clone()).unwrap_or_default();
        let (targets, env) = (self.run_targets(), self.env.clone());
        let panel = cx.new(|cx| RunOptionsPanel::new(targets, &initial, env, window, cx));
        let this = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let (panel_c, this) = (panel.clone(), this.clone());
            dialog
                .title("运行配置")
                .w(px(640.))
                .child(panel.clone())
                .footer(
                    h_flex()
                        .gap_2()
                        .justify_end()
                        .child(Button::new("run-cancel").outline().label("取消").on_click(|_, window, cx| window.close_dialog(cx)))
                        .child(Button::new("run-go").primary().icon(IconName::Play).label("运行").on_click(move |_, window, cx| {
                            let req = panel_c.read(cx).request(cx);
                            window.close_dialog(cx);
                            let _ = this.update(cx, |w, cx| {
                                w.last_request = Some(req.clone());
                                w.start_job(RunJob::Run(req), vec![], window, cx);
                            });
                        })),
                )
        });
    }

    fn on_run_event(&mut self, _: &Entity<RunState>, ev: &RunStateEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            RunStateEvent::CaseDone { file } => {
                if self.editor.read(cx).rel() == Some(file.as_str()) {
                    self.show_session_response(&file.clone(), window, cx);
                }
            }
            RunStateEvent::Finished { send, workflow, failed, total, dry_run } => {
                if *send {
                    return;
                }
                if !*dry_run {
                    self.reload_history(cx);
                }
                let what = workflow.as_ref().map(|w| format!("工作流「{w}」")).unwrap_or_else(|| "运行".into());
                let note = if *dry_run {
                    Notification::info(format!("{what} dry-run 完成：{total} 个用例编译通过，未发送请求"))
                } else if *failed == 0 {
                    Notification::success(format!("{what}：{total} 个用例全部通过，已写入共享历史"))
                } else {
                    Notification::warning(format!("{what}：{failed}/{total} 个用例失败 —— 在结果中查看现场"))
                };
                window.push_notification(note, cx);
            }
            RunStateEvent::Error(msg) => window.push_notification(Notification::error(msg.clone()), cx),
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

    fn export_report(&mut self, html: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(report) = self.run.read(cx).report().cloned() else {
            window.push_notification(Notification::warning("当前没有可导出的运行报告"), cx);
            return;
        };
        let id = report.meta.as_ref().map(|m| m.id.clone()).unwrap_or_else(|| "report".into());
        let name = format!("requestbaby-{id}.{}", if html { "html" } else { "json" });
        let rx = cx.prompt_for_new_path(self.project.root(), Some(&name));
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(path))) = rx.await else { return };
            let text = if html { report::render_html(&report) } else { report::render_json(&report) };
            let note = match std::fs::write(&path, text) {
                Ok(()) => Notification::success(format!("报告已导出：{}", path.display())),
                Err(e) => Notification::error(format!("导出失败：{e}")),
            };
            let _ = cx.update(|window, cx| window.push_notification(note, cx));
        })
        .detach();
    }

    // ---------------------------------------------------------------- 校验 / 格式化 / 自检

    fn revalidate(&mut self, cx: &mut Context<Self>) {
        let (dir, env) = (self.project.collections_dir(), self.env.clone());
        self.validation.running = true;
        let task = cx.background_spawn(async move { validate::validate_paths(&[dir], &env) });
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
        let preview = match fmt::format_paths(std::slice::from_ref(&dir), true) {
            Ok(r) => r,
            Err(e) => {
                window.push_notification(Notification::error(format!("{e:#}")), cx);
                return;
            }
        };
        let n = preview.case_changes();
        if n == 0 {
            window.push_notification(Notification::success("所有用例已是规范格式（requestbaby fmt --check 通过）"), cx);
            return;
        }
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (this, dir) = (this.clone(), dir.clone());
            alert
                .title(format!("格式化 {n} 个用例？"))
                .description(
                    "字段按可读顺序排列、清理默认值；目录内所有用例相同的请求头上提到 collection.json。\n等价命令：requestbaby fmt",
                )
                .confirm()
                .ok_text("格式化")
                .cancel_text("取消")
                .on_ok(move |_, window, cx| {
                    match fmt::format_paths(std::slice::from_ref(&dir), false) {
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
        dialogs::show_doctor(doctor::run(self.project.root()), window, cx);
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
        let history_changed = paths.iter().any(|p| p.starts_with(&state_dir));
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
        let Some(target) = self.center_target(cx).filter(|_| !matches!(self.center, Center::Case)) else { return };
        let path = match &self.center {
            Center::Collection(_, e) => e.read(cx).path().clone(),
            Center::Env(_, e) => e.read(cx).path().clone(),
            Center::Project(e) => e.read(cx).path().clone(),
            Center::Case => return,
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
        let dark = cx.theme().mode.is_dark();
        let view = cx.entity().downgrade();
        let root = self.project.root().to_path_buf();
        TitleBar::new()
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
                                    .item(menu_item(&view, "校验项目", IconName::CircleCheck, |w, _, cx| {
                                        w.bottom_tab = BottomTab::Validate;
                                        w.revalidate(cx);
                                    }))
                                    .item(menu_item(&view, "格式化项目", IconName::FileText, |w, window, cx| w.format_project(window, cx)))
                                    .item(menu_item(&view, "环境自检", IconName::Settings, |w, window, cx| w.run_doctor(window, cx)))
                                    .item(menu_item(&view, "项目设置", IconName::Settings2, |w, window, cx| w.open(Target::Project, window, cx)))
                                    .separator()
                                    .item(PopupMenuItem::new("在 Finder 中显示").icon(IconName::Folder).on_click(move |_, _, cx| cx.reveal_path(&root)))
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
                                    .primary()
                                    .icon(IconName::Play)
                                    .label("运行全部")
                                    .tooltip("requestbaby run collections（⌘R）")
                                    .on_click(cx.listener(|this, _, window, cx| this.run_all(window, cx))),
                            )
                            .child(
                                Button::new("run-options")
                                    .small()
                                    .outline()
                                    .icon(IconName::Settings2)
                                    .label("运行配置")
                                    .tooltip("选择目标、标签、并发、dry-run…（⇧⌘R）")
                                    .on_click(cx.listener(|this, _, window, cx| this.open_run_dialog(None, window, cx))),
                            )
                        }
                    })
                    .child(
                        Button::new("toggle-theme")
                            .small()
                            .ghost()
                            .icon(if dark { IconName::Sun } else { IconName::Moon })
                            .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleTheme), cx)),
                    ),
            )
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
            Center::Env(_, e) => div().size_full().child(e.clone()).into_any_element(),
            Center::Project(e) => div().size_full().child(e.clone()).into_any_element(),
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
            .on_action(cx.listener(|this, _: &RunAll, window, cx| this.run_all(window, cx)))
            .on_action(cx.listener(|this, _: &RunWithOptions, window, cx| this.open_run_dialog(None, window, cx)))
            .on_action(cx.listener(|this, _: &StopRun, _, cx| this.run.update(cx, |r, cx| r.stop(cx))))
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_title_bar(cx))
            .child(
                div().flex_1().min_h_0().child(
                    h_resizable("main-split")
                        .child(
                            resizable_panel()
                                .size(px(300.))
                                .size_range(px(220.)..px(460.))
                                .child(self.render_sidebar(cx)),
                        )
                        .child(
                            v_resizable("work-split")
                                .child(resizable_panel().child(self.render_center(cx)))
                                .child(
                                    resizable_panel()
                                        .size(px(280.))
                                        .size_range(px(140.)..px(640.))
                                        .child(self.render_bottom(cx)),
                                ),
                        ),
                ),
            )
            .child(self.render_status_bar(cx))
    }
}

/// 打开某文档会丢弃未保存修改时的确认框
pub(crate) fn confirm_discard(lost: Vec<String>, window: &mut Window, cx: &mut App, then: impl Fn(&mut Window, &mut App) + 'static) {
    let then = std::rc::Rc::new(then);
    window.open_alert_dialog(cx, move |alert, _, _| {
        let then = then.clone();
        alert
            .title(format!("放弃对「{}」的修改？", lost.join("」「")))
            .description("修改尚未写入文件。")
            .confirm()
            .ok_text("放弃修改")
            .ok_variant(ButtonVariant::Danger)
            .cancel_text("继续编辑")
            .on_ok(move |_, window, cx| {
                then(window, cx);
                true
            })
    });
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
