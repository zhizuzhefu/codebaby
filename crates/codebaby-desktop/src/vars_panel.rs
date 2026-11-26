//! 变量面板：变量的唯一编辑入口。按层分标签（总览 / 项目 / 各环境 / 各集合），
//! 顶部常驻「正在编辑哪一层、哪个文件、优先级链」，避免改错层。
//!
//! 读写全部走 core::varstore，与 `codebaby vars` / `vars set` 同一实现。
use crate::dialogs;
use crate::vars_editor::{VarsEditor, VarsEditorEvent};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex,
    button::{Button, ButtonVariants as _},
    tab::{Tab, TabBar},
    tag::Tag,
    tooltip::Tooltip,
    v_flex,
};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
};
use codebaby_core::varstore::{self, VarScope, VarsMatrix};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::PathBuf;

pub enum VarsPanelEvent {
    Saved,
    NewEnv,
    DuplicateEnv(String),
    DeleteEnv(String),
}

pub struct VarsPanel {
    root: PathBuf,
    current_env: String,
    default_env: String,
    matrix: Option<VarsMatrix>,
    error: Option<String>,
    /// 标签页（总览之后）：项目、各环境、定义了变量的集合，以及当前打开的其他集合
    scopes: Vec<VarScope>,
    /// None = 总览
    active: Option<VarScope>,
    editor: Entity<VarsEditor>,
    /// 当前层加载时的变量表：判断是否有未保存修改
    original: Map<String, Value>,
    dirty: bool,
    /// 嵌入运行配置对话框时限制高度
    compact: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<VarsPanelEvent> for VarsPanel {}

impl VarsPanel {
    pub fn new(
        root: PathBuf,
        current_env: String,
        default_env: String,
        initial: Option<VarScope>,
        compact: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = cx.new(|_| VarsEditor::new(true));
        let subs = vec![cx.subscribe(&editor, |this, _, _: &VarsEditorEvent, cx| this.refresh_dirty(cx))];
        let mut this = Self {
            root,
            current_env,
            default_env,
            matrix: None,
            error: None,
            scopes: vec![],
            active: None,
            editor,
            original: Map::new(),
            dirty: false,
            compact,
            _subscriptions: subs,
        };
        this.reload_matrix();
        this.select_now(initial, window, cx);
        this
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// 有未保存修改时的文档名（关闭 / 切换前确认）
    pub fn dirty_label(&self) -> Option<String> {
        let scope = self.active.as_ref()?;
        self.dirty.then(|| format!("{}的变量", scope.label()))
    }

    pub fn active(&self) -> Option<&VarScope> {
        self.active.as_ref()
    }

    fn reload_matrix(&mut self) {
        match varstore::matrix(&self.root) {
            Ok(m) => {
                // 只列当前环境的层：其他环境的变量要先切换到那个环境再编辑
                let mut scopes: Vec<VarScope> = m.columns.iter().map(|c| c.scope.clone()).filter(|s| in_env(s, &self.current_env)).collect();
                if let Some(a) = self.active.as_ref().filter(|a| in_env(a, &self.current_env)) {
                    if !scopes.contains(a) {
                        scopes.push(a.clone());
                    }
                }
                self.scopes = scopes;
                self.matrix = Some(m);
                self.error = None;
            }
            Err(e) => self.error = Some(format!("{e:#}")),
        }
    }

    /// 运行环境变化：正在编辑的环境层换成新环境的同一层（未保存的修改先保存），标签只剩新环境的
    pub fn set_env(&mut self, current_env: String, default_env: String, window: &mut Window, cx: &mut Context<Self>) {
        self.default_env = default_env;
        if current_env == self.current_env {
            cx.notify();
            return;
        }
        if self.dirty && !self.save(window, cx) {
            return;
        }
        let local_exists = codebaby_core::config::local_env_path(&self.root, &current_env).exists();
        self.active = match self.active.take() {
            Some(VarScope::Env(_)) => Some(VarScope::Env(current_env.clone())),
            Some(VarScope::Local(_)) if local_exists => Some(VarScope::Local(current_env.clone())),
            Some(VarScope::Local(_)) => Some(VarScope::Env(current_env.clone())),
            other => other,
        };
        self.current_env = current_env;
        self.reload_matrix();
        self.load_active(window, cx);
    }

    /// 切换到某层（None = 总览）；当前层有未保存修改时先确认
    pub fn select(&mut self, scope: Option<VarScope>, window: &mut Window, cx: &mut Context<Self>) {
        if scope == self.active {
            return;
        }
        match self.dirty_label() {
            None => self.select_now(scope, window, cx),
            Some(label) => {
                let this = cx.entity().downgrade();
                dialogs::confirm_discard(vec![label], window, cx, move |window, cx| {
                    let _ = this.update(cx, |p, cx| p.select_now(scope.clone(), window, cx));
                });
            }
        }
    }

    fn select_now(&mut self, scope: Option<VarScope>, window: &mut Window, cx: &mut Context<Self>) {
        // 其他环境的层不在这里显示（调用方应先切换环境）
        let scope = scope.filter(|s| in_env(s, &self.current_env));
        if let Some(s) = &scope {
            if !self.scopes.contains(s) {
                self.scopes.push(s.clone());
            }
        }
        self.active = scope;
        self.load_active(window, cx);
    }

    fn load_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.original = match &self.active {
            Some(scope) => match varstore::read_raw(&self.root, scope) {
                Ok(m) => m,
                Err(e) => {
                    self.error = Some(format!("{e:#}"));
                    Map::new()
                }
            },
            None => Map::new(),
        };
        let original = self.original.clone();
        self.editor.update(cx, |e, cx| e.load(Some(&original), window, cx));
        self.dirty = false;
        self.refresh_hints(cx);
        cx.notify();
    }

    /// 在某层新增变量（如用例中未定义的变量），预填变量名并聚焦值
    pub fn add_var(&mut self, scope: VarScope, name: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.active.as_ref() != Some(&scope) {
            if self.dirty {
                // 不静默丢弃：先切过去（会弹确认），新增由用户在目标层完成
                self.select(Some(scope), window, cx);
                return;
            }
            self.select_now(Some(scope), window, cx);
        }
        let exists = self.original.contains_key(&name);
        if !exists {
            self.editor.update(cx, |e, cx| e.add_named(name, window, cx));
        }
    }

    /// 文件在外部（CLI / AI / 其他编辑器）变化后刷新；当前层有未保存修改时只刷新总览
    pub fn reload_if_clean(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reload_matrix();
        if !self.dirty {
            self.load_active(window, cx);
        } else {
            self.refresh_hints(cx);
        }
        cx.notify();
    }

    fn refresh_dirty(&mut self, cx: &mut Context<Self>) {
        let dirty = self.active.is_some() && self.editor.read(cx).to_map(cx) != self.original;
        if dirty != self.dirty {
            self.dirty = dirty;
            cx.notify();
        }
    }

    pub fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(scope) = self.active.clone() else { return true };
        if !self.dirty {
            return true;
        }
        let map = self.editor.read(cx).to_map(cx);
        match varstore::write_all(&self.root, &scope, map) {
            Ok(_) => {
                self.reload_matrix();
                self.load_active(window, cx);
                cx.emit(VarsPanelEvent::Saved);
                true
            }
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                cx.notify();
                false
            }
        }
    }

    /// 每个变量与其他层的关系：覆盖了谁、被谁覆盖、运行时是否被捕获
    fn refresh_hints(&mut self, cx: &mut Context<Self>) {
        let hints = match (&self.matrix, &self.active) {
            (Some(m), Some(active)) => hints_for(m, active),
            _ => HashMap::new(),
        };
        self.editor.update(cx, |e, cx| e.set_hints(hints, cx));
    }

    fn tab_label(&self, scope: &VarScope) -> String {
        match scope {
            VarScope::Project => "项目变量".into(),
            other => other.label(),
        }
    }

    // ---------------------------------------------------------------- 渲染

    fn render_banner(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let (title, file, desc): (String, Option<String>, String) = match &self.active {
            None => (
                "变量总览".into(),
                None,
                format!("每行一个变量、每列一层；当前运行环境 {}。点击格子即可编辑该层（空格子 = 在该层新增）", self.current_env),
            ),
            Some(scope) => {
                let file = scope.file(&self.root);
                let rel = file.strip_prefix(&self.root).unwrap_or(&file).display().to_string();
                let desc = match scope {
                    VarScope::Project => "项目变量：所有环境共享，优先级最低；环境 / 集合 / 运行时的同名变量会覆盖它".to_string(),
                    VarScope::Env(name) => format!("环境变量：环境 {name} 运行时生效，覆盖项目变量"),
                    VarScope::Collection(dir) => format!("集合变量：只对 {dir}/ 下的用例生效；覆盖环境与项目变量"),
                    VarScope::Local(name) => format!(
                        "本机覆盖：只在这台电脑上、环境 {name} 运行时覆盖同名环境变量；文件已加入 .gitignore，适合放 token / 本机端口，不会提交"
                    ),
                };
                (format!("正在编辑：{}", scope.label()), Some(rel), desc)
            }
        };
        // 优先级链：低 → 高，高亮当前层
        let level = match &self.active {
            Some(VarScope::Project) => Some(0),
            Some(VarScope::Env(_)) => Some(1),
            Some(VarScope::Local(_)) => Some(2),
            Some(VarScope::Collection(_)) => Some(3),
            None => None,
        };
        let chain = ["项目", "环境", "本机覆盖", "集合（浅 → 深）", "数据驱动", "运行时捕获"];
        v_flex()
            .px_4()
            .py_2()
            .gap_1()
            .bg(cx.theme().muted.opacity(0.5))
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .gap_2()
                    .child(Icon::new(if self.active.is_some() { IconName::Settings2 } else { IconName::LayoutDashboard }).small())
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(title))
                    .when_some(file, |this, f| {
                        this.child(div().text_xs().font_family(cx.theme().mono_font_family.clone()).text_color(muted).child(f))
                    })
                    .when(self.dirty, |this| this.child(Tag::warning().outline().xsmall().child("未保存"))),
            )
            .child(div().text_xs().text_color(muted).child(desc))
            .child(
                h_flex().gap_1().text_xs().children(chain.iter().enumerate().flat_map(|(ix, name)| {
                    let active = level == Some(ix);
                    let chip = div()
                        .px_1p5()
                        .rounded(cx.theme().radius)
                        .when(active, |d| d.bg(cx.theme().primary).text_color(cx.theme().primary_foreground))
                        .when(!active, |d| d.text_color(muted))
                        .child(*name)
                        .into_any_element();
                    let sep = (ix + 1 < chain.len()).then(|| div().text_color(muted).child("<").into_any_element());
                    std::iter::once(chip).chain(sep)
                })),
            )
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let env_actions = match &self.active {
            Some(VarScope::Env(name)) => Some((name.clone(), *name == self.default_env)),
            _ => None,
        };
        let cli = match &self.active {
            Some(VarScope::Project) => Some("codebaby vars set <名称> <值> --project".to_string()),
            Some(VarScope::Env(n)) => Some(format!("codebaby vars set <名称> <值> -e {n}")),
            Some(VarScope::Collection(d)) => Some(format!("codebaby vars set <名称> <值> --collection {d}")),
            Some(VarScope::Local(n)) => Some(format!("codebaby vars set <名称> <值> --local {n}")),
            None => Some("codebaby vars".to_string()),
        };
        h_flex()
            .px_4()
            .py_2()
            .gap_2()
            .when_some(cli, |this, c| this.child(crate::style::cli_button("vars-cli", c)))
            .child(div().flex_1())
            // 对话框内只编辑变量：环境的增删在主界面完成
            .when(!self.compact, |this| {
                this.child(
                    Button::new("vars-new-env")
                        .small()
                        .ghost()
                        .icon(IconName::Plus)
                        .label("新建环境")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(VarsPanelEvent::NewEnv))),
                )
            })
            .when_some(env_actions.clone(), |this, (name, _)| {
                let has_local = codebaby_core::config::local_env_path(&self.root, &name).exists();
                this.child(
                    Button::new("vars-local-env")
                        .small()
                        .ghost()
                        .icon(IconName::HardDrive)
                        .label(if has_local { "本机覆盖" } else { "添加本机覆盖" })
                        .tooltip(format!("envs/{name}.local.json：只在本机生效、不入库（自动加入 .gitignore），放 token / 本机端口"))
                        .on_click(cx.listener(move |this, _, window, cx| this.select(Some(VarScope::Local(name.clone())), window, cx))),
                )
            })
            .when_some(env_actions.filter(|_| !self.compact), |this, (name, is_default)| {
                let (n1, n2) = (name.clone(), name);
                this.child(
                    Button::new("vars-dup-env")
                        .small()
                        .ghost()
                        .icon(IconName::Copy)
                        .label("复制为新环境")
                        .on_click(cx.listener(move |_, _, _, cx| cx.emit(VarsPanelEvent::DuplicateEnv(n1.clone())))),
                )
                .child(
                    Button::new("vars-del-env")
                        .small()
                        .ghost()
                        .icon(IconName::Delete)
                        .disabled(is_default)
                        .tooltip(if is_default { "默认环境不能删除" } else { "删除环境文件" })
                        .on_click(cx.listener(move |_, _, _, cx| cx.emit(VarsPanelEvent::DeleteEnv(n2.clone())))),
                )
            })
            .when(self.active.is_some(), |this| {
                this.child(
                    Button::new("vars-save")
                        .small()
                        .primary()
                        .icon(IconName::Check)
                        .label("保存")
                        .disabled(!self.dirty)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.save(window, cx);
                        })),
                )
            })
    }

    fn render_matrix(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(m) = &self.matrix else {
            return div().p_4().text_xs().text_color(cx.theme().danger).children(self.error.clone()).into_any_element();
        };
        if m.rows.is_empty() {
            return div()
                .p_4()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("还没有任何变量：选择上方的「项目变量」或某个环境开始添加")
                .into_any_element();
        }
        let muted = cx.theme().muted_foreground;
        // 只展示项目 / 当前环境（含本机覆盖）/ 集合这几层：其他环境此刻不生效
        let shown: Vec<usize> = (0..m.columns.len()).filter(|&i| in_env(&m.columns[i].scope, &self.current_env)).collect();
        let current_col = m.columns.iter().position(|c| c.scope == VarScope::Env(self.current_env.clone()));
        let local_col = m.columns.iter().position(|c| c.scope == VarScope::Local(self.current_env.clone()));
        let col_w = px(170.);
        let head = h_flex()
            .px_2()
            .py_1()
            .gap_2()
            .text_xs()
            .text_color(cx.theme().table_head_foreground)
            .bg(cx.theme().table_head)
            .child(div().w(px(160.)).flex_shrink_0().child("变量"))
            .children(m.columns.iter().enumerate().filter(|(ix, _)| shown.contains(ix)).map(|(ix, c)| {
                let current = Some(ix) == current_col || Some(ix) == local_col;
                let label = if current { format!("{} · 当前", c.label) } else { c.label.clone() };
                div()
                    .w(col_w)
                    .flex_shrink_0()
                    .when(current, |d| d.text_color(cx.theme().primary).font_weight(FontWeight::SEMIBOLD))
                    .child(label)
            }))
            .child(div().w(px(110.)).flex_shrink_0().child("运行时捕获"))
            .child(div().flex_1().child("引用"));
        let rows = m.rows.iter().enumerate().map(|(rix, r)| {
            // 当前环境下（不计集合，集合只对其下用例生效）实际生效的层
            let effective = [local_col, current_col, Some(0)].into_iter().flatten().find(|&i| r.cells.get(i).is_some_and(Option::is_some));
            // 当前环境下没有任何一层定义（其他环境定义了也不算）
            let undefined = shown.iter().all(|&i| r.cells[i].is_none()) && r.runtime_writers.is_empty();
            h_flex()
                .px_2()
                .py_1()
                .gap_2()
                .text_xs()
                .border_b_1()
                .border_color(cx.theme().table_row_border)
                .child(
                    div()
                        .w(px(160.))
                        .flex_shrink_0()
                        .font_family(cx.theme().mono_font_family.clone())
                        .when(undefined, |d| d.text_color(cx.theme().danger))
                        .child(r.name.clone()),
                )
                .children(m.columns.iter().enumerate().filter(|(cix, _)| shown.contains(cix)).map(|(cix, col)| {
                    let scope = col.scope.clone();
                    let name = r.name.clone();
                    let (text, defined) = match &r.cells[cix] {
                        Some(d) => (d.display(&r.name), true),
                        None => ("＋".to_string(), false),
                    };
                    div()
                        .id(("var-cell", rix * 1000 + cix))
                        .w(col_w)
                        .flex_shrink_0()
                        .px_1()
                        .rounded(cx.theme().radius)
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .cursor_pointer()
                        .hover(|d| d.bg(cx.theme().table_hover))
                        .map(|d| match (defined, effective == Some(cix)) {
                            (true, true) => d.font_weight(FontWeight::SEMIBOLD).text_color(cx.theme().foreground),
                            (true, false) => d.text_color(muted),
                            (false, _) => d.text_color(muted.opacity(0.5)),
                        })
                        .tooltip(move |window, cx| {
                            Tooltip::new(if defined { "点击编辑这一层" } else { "点击在这一层新增" }).build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if defined {
                                this.select(Some(scope.clone()), window, cx);
                            } else {
                                this.add_var(scope.clone(), name.clone(), window, cx);
                            }
                        }))
                        .child(text)
                }))
                .child(
                    div()
                        .w(px(110.))
                        .flex_shrink_0()
                        .text_color(if r.runtime_writers.is_empty() { muted } else { cx.theme().warning })
                        .child(if r.runtime_writers.is_empty() { "—".to_string() } else { format!("{} 处写入", r.runtime_writers.len()) }),
                )
                .child(div().flex_1().text_color(if undefined { cx.theme().danger } else { muted }).child(match (undefined, r.used_by.len()) {
                    (true, n) => format!("未定义 · {n} 个用例引用"),
                    (false, 0) => "未被引用".to_string(),
                    (false, n) => format!("{n} 个用例"),
                }))
        });
        v_flex().child(head).children(rows).into_any_element()
    }
}

impl Render for VarsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut labels = vec!["总览".to_string()];
        labels.extend(self.scopes.iter().map(|s| self.tab_label(s)));
        let selected = match &self.active {
            None => 0,
            Some(a) => self.scopes.iter().position(|s| s == a).map(|i| i + 1).unwrap_or(0),
        };
        let body: AnyElement = match &self.active {
            None => self.render_matrix(cx),
            Some(_) => v_flex()
                .gap_2()
                .child(self.editor.clone())
                .when_some(self.error.clone(), |this, e| this.child(div().text_xs().text_color(cx.theme().danger).child(e)))
                .into_any_element(),
        };
        v_flex()
            .size_full()
            .child(
                div().px_4().pt_2().child(
                    TabBar::new("vars-layers")
                        .underline()
                        .small()
                        .selected_index(selected)
                        .children(labels.into_iter().map(|l| Tab::new().label(l)))
                        .on_click(cx.listener(|this, ix: &usize, window, cx| {
                            let scope = if *ix == 0 { None } else { this.scopes.get(ix - 1).cloned() };
                            this.select(scope, window, cx);
                        })),
                ),
            )
            .child(self.render_banner(cx))
            .child(
                div()
                    .id("vars-body")
                    .flex_1()
                    .min_h_0()
                    .when(self.compact, |d| d.max_h(px(320.)))
                    .overflow_y_scroll()
                    .p_4()
                    .child(body),
            )
            .child(div().border_t_1().border_color(cx.theme().border).child(self.render_toolbar(cx)))
    }
}

/// 该层在环境 env 下是否生效：其他环境（及其本机覆盖）不生效
fn in_env(scope: &VarScope, env: &str) -> bool {
    match scope {
        VarScope::Env(n) | VarScope::Local(n) => n == env,
        _ => true,
    }
}

/// 某层变量与其他层的关系提示
fn hints_for(m: &VarsMatrix, active: &VarScope) -> HashMap<String, SharedString> {
    let Some(ai) = m.columns.iter().position(|c| &c.scope == active) else { return HashMap::new() };
    let mut out = HashMap::new();
    for r in &m.rows {
        if r.cells.get(ai).is_none_or(Option::is_none) {
            continue;
        }
        let mut lower = vec![];
        let mut higher = vec![];
        for (ix, c) in m.columns.iter().enumerate() {
            if ix == ai || r.cells[ix].is_none() {
                continue;
            }
            // 不同环境互斥（一次只用一个），不构成覆盖关系
            let env_of = |s: &VarScope| match s {
                VarScope::Env(n) | VarScope::Local(n) => Some(n.clone()),
                _ => None,
            };
            if let (Some(a), Some(b)) = (env_of(&c.scope), env_of(active)) {
                if a != b {
                    continue;
                }
            }
            let short = match &c.scope {
                VarScope::Project => "项目".to_string(),
                other => other.label(),
            };
            if ix < ai { lower.push(short) } else { higher.push(short) }
        }
        let mut parts = vec![];
        if !lower.is_empty() {
            parts.push(format!("覆盖 {}", lower.join("、")));
        }
        if !higher.is_empty() {
            parts.push(format!("被 {} 覆盖", higher.join("、")));
        }
        if !r.runtime_writers.is_empty() {
            parts.push("运行时被捕获覆盖".into());
        }
        if r.used_by.is_empty() {
            parts.push("未被引用".into());
        }
        if !parts.is_empty() {
            out.insert(r.name.clone(), SharedString::from(parts.join(" · ")));
        }
    }
    out
}
