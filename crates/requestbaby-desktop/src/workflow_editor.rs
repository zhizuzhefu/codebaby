//! 流程编辑器：一个流程一个页面——key / 显示名 / 依赖的流程（如登录）/ 有序步骤（增删、上下移动、目标 / 标签 / 用例名过滤 / 以哪个用户运行）。
//!
//! 读写走 core 的 `workflow::{get, save, delete, duplicate}`（与 CLI `requestbaby workflow …` 同一实现），
//! 只改 requestbaby.json 的 workflows 字段；项目设置等其他编辑器不会覆盖这里的修改。
use crate::dialogs;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, IndexPath, Sizable as _, h_flex,
    button::{Button, ButtonVariants as _},
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    select::{Select, SelectEvent, SelectState},
    tag::Tag,
    v_flex,
};
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, prelude::FluentBuilder as _,
    px,
};
use requestbaby_core::config::{Workflow, WorkflowStep};
use requestbaby_core::workflow;
use std::path::PathBuf;

/// 运行环境下拉的第一项：不固定
const FOLLOW_ENV: &str = "跟随当前环境";

pub enum WorkflowEditorEvent {
    /// 已保存（新 key）
    Saved(String),
    /// 已删除：关闭编辑器
    Deleted(String),
    /// 运行（已先保存）
    Run { key: String, dry_run: bool },
    /// 已复制为新流程：打开它
    Duplicated(String),
}

struct StepRow {
    id: usize,
    run: Entity<SelectState<Vec<SharedString>>>,
    name: Entity<InputState>,
    tags: Entity<InputState>,
    name_filter: Entity<InputState>,
    /// 以哪个用户运行（空 = 默认用户）：每个用户有独立的登录态
    user: Entity<InputState>,
}

pub struct WorkflowEditor {
    root: PathBuf,
    /// 磁盘上的 key（None = 尚未保存的新流程）
    original: Option<String>,
    /// 上次保存的内容（判定未保存修改）
    saved: Option<(String, Workflow)>,
    key: Entity<InputState>,
    name: Entity<InputState>,
    /// 运行环境：第一项 = 跟随当前环境，其余为固定环境
    env: Entity<SelectState<Vec<SharedString>>>,
    /// 依赖的流程（先运行它们，登录态沿用到本流程所有步骤）
    needs: Vec<String>,
    steps: Vec<StepRow>,
    targets: Vec<SharedString>,
    error: Option<String>,
    next_id: usize,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<WorkflowEditorEvent> for WorkflowEditor {}

impl WorkflowEditor {
    /// `key` 为 None 时新建：预填一个不重名的 key 与一个步骤
    pub fn new(root: PathBuf, key: Option<String>, targets: Vec<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let loaded = key.as_ref().and_then(|k| workflow::get(&root, k).ok().map(|w| (k.clone(), w)));
        let (key_text, wf) = match &loaded {
            Some((k, w)) => (k.clone(), w.clone()),
            None => {
                let taken: Vec<String> = workflow::list(&root).map(|l| l.into_iter().map(|(k, _)| k).collect()).unwrap_or_default();
                let key = (1..).map(|n| format!("workflow{n}")).find(|k| !taken.contains(k)).unwrap_or_default();
                let first = targets.first().cloned().unwrap_or_else(|| "collections".into());
                (key, Workflow { name: String::new(), env: None, needs: vec![], steps: vec![WorkflowStep { run: first, ..Default::default() }] })
            }
        };
        let key_input = cx.new(|cx| InputState::new(window, cx).placeholder("流程 key（英文，用于运行与引用）").default_value(key_text));
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("显示名（可选）").default_value(wf.name.clone()));
        let mut env_items: Vec<SharedString> = vec![FOLLOW_ENV.into()];
        env_items.extend(requestbaby_core::config::list_envs(&root).into_iter().map(SharedString::from));
        let env_ix = wf.env.as_ref().and_then(|e| env_items.iter().position(|x| x.as_ref() == e.as_str())).unwrap_or(0);
        let env_select = cx.new(|cx| SelectState::new(env_items, Some(IndexPath::new(env_ix)), window, cx));
        let mut this = Self {
            root,
            original: loaded.as_ref().map(|(k, _)| k.clone()),
            saved: loaded,
            key: key_input.clone(),
            name: name_input.clone(),
            env: env_select.clone(),
            needs: wf.needs.clone(),
            steps: vec![],
            targets: targets.into_iter().map(SharedString::from).collect(),
            error: None,
            next_id: 0,
            _subscriptions: vec![],
        };
        this.watch_input(&key_input, window, cx);
        this.watch_input(&name_input, window, cx);
        this._subscriptions.push(cx.subscribe_in(&env_select, window, |this, _, _: &SelectEvent<Vec<SharedString>>, _, cx| {
            this.error = None;
            cx.notify();
        }));
        for step in &wf.steps {
            this.push_step(step, None, window, cx);
        }
        this
    }

    pub fn key(&self) -> Option<&str> {
        self.original.as_deref()
    }

    pub fn is_dirty(&self, cx: &gpui_kit::App) -> bool {
        match &self.saved {
            Some((k, w)) => self.collect(cx) != (k.clone(), w.clone()),
            None => true,
        }
    }

    /// 未保存时的文档名（关闭 / 切换前确认）
    pub fn dirty_label(&self, cx: &gpui_kit::App) -> Option<String> {
        self.is_dirty(cx).then(|| format!("流程 {}", self.key.read(cx).value().trim()))
    }

    /// 外部修改了 requestbaby.json（CLI / AI）：没有未保存的编辑时同步磁盘内容
    pub fn reload_if_clean(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.original.clone() else { return };
        if self.is_dirty(cx) {
            return;
        }
        match workflow::get(&self.root, &key) {
            Ok(wf) if self.saved.as_ref().is_some_and(|(_, w)| *w == wf) => {}
            Ok(wf) => {
                self.name.update(cx, |s, cx| s.set_value(wf.name.clone(), window, cx));
                let env: SharedString = wf.env.clone().unwrap_or_else(|| FOLLOW_ENV.into()).into();
                self.env.update(cx, |s, cx| s.set_selected_value(&env, window, cx));
                self.needs = wf.needs.clone();
                self.steps.clear();
                for step in &wf.steps {
                    self.push_step(step, None, window, cx);
                }
                self.saved = Some((key, wf));
                cx.notify();
            }
            Err(_) => {}
        }
    }

    pub fn set_targets(&mut self, targets: Vec<String>) {
        self.targets = targets.into_iter().map(SharedString::from).collect();
    }

    fn watch_input(&mut self, input: &Entity<InputState>, window: &mut Window, cx: &mut Context<Self>) {
        self._subscriptions.push(cx.subscribe_in(input, window, |this, _, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::Change) {
                this.error = None;
                cx.notify();
            }
        }));
    }

    fn input(&mut self, value: &str, placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        let value = value.to_string();
        let e = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(value));
        self.watch_input(&e, window, cx);
        e
    }

    fn push_step(&mut self, step: &WorkflowStep, at: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.next_id += 1;
        let id = self.next_id;
        let current = SharedString::from(step.run.clone());
        let mut items = self.targets.clone();
        if !current.is_empty() && !items.contains(&current) {
            items.push(current.clone());
        }
        let ix = items.iter().position(|t| *t == current).map(IndexPath::new);
        let run = cx.new(|cx| SelectState::new(items, ix, window, cx).searchable(true));
        self._subscriptions.push(cx.subscribe_in(&run, window, |this, _, _: &SelectEvent<Vec<SharedString>>, _, cx| {
            this.error = None;
            cx.notify();
        }));
        let row = StepRow {
            id,
            run,
            name: self.input(&step.name, "步骤名（可选）", window, cx),
            tags: self.input(&step.tags.join(", "), "标签，逗号分隔", window, cx),
            name_filter: self.input(&step.name_filter, "用例名包含…", window, cx),
            user: self.input(step.as_user.as_deref().unwrap_or(""), "默认用户", window, cx),
        };
        let at = at.unwrap_or(self.steps.len()).min(self.steps.len());
        self.steps.insert(at, row);
    }

    fn collect(&self, cx: &gpui_kit::App) -> (String, Workflow) {
        let text = |e: &Entity<InputState>| e.read(cx).value().trim().to_string();
        let steps = self
            .steps
            .iter()
            .map(|s| WorkflowStep {
                name: text(&s.name),
                run: s.run.read(cx).selected_value().map(|v| v.to_string()).unwrap_or_default(),
                tags: text(&s.tags).split(',').map(str::trim).filter(|t| !t.is_empty()).map(String::from).collect(),
                name_filter: text(&s.name_filter),
                as_user: Some(text(&s.user)).filter(|u| !u.is_empty()),
            })
            .collect();
        let env = self.env.read(cx).selected_value().map(|v| v.to_string()).filter(|v| v != FOLLOW_ENV);
        (text(&self.key), Workflow { name: text(&self.name), env, needs: self.needs.clone(), steps })
    }

    pub fn save(&mut self, cx: &mut Context<Self>) -> bool {
        let (key, wf) = self.collect(cx);
        match workflow::save(&self.root, self.original.as_deref(), &key, &wf) {
            Ok(()) => {
                self.original = Some(key.clone());
                self.saved = Some((key.clone(), wf));
                self.error = None;
                cx.emit(WorkflowEditorEvent::Saved(key));
                cx.notify();
                true
            }
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                cx.notify();
                false
            }
        }
    }

    fn run(&mut self, dry_run: bool, cx: &mut Context<Self>) {
        if (self.is_dirty(cx) || self.original.is_none()) && !self.save(cx) {
            return;
        }
        if let Some(key) = self.original.clone() {
            cx.emit(WorkflowEditorEvent::Run { key, dry_run });
        }
    }

    fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.original.clone() else {
            // 尚未保存：直接丢弃
            cx.emit(WorkflowEditorEvent::Deleted(String::new()));
            return;
        };
        let this = cx.entity().downgrade();
        let root = self.root.clone();
        dialogs::confirm_danger(
            window,
            cx,
            format!("删除流程 {key}？"),
            "只删除 requestbaby.json 中的编排，不会删除任何用例文件。",
            "删除",
            move |_, cx| {
                let _ = this.update(cx, |this, cx| match workflow::delete(&root, &key) {
                    Ok(()) => cx.emit(WorkflowEditorEvent::Deleted(key.clone())),
                    Err(e) => {
                        this.error = Some(format!("{e:#}"));
                        cx.notify();
                    }
                });
            },
        );
    }

    fn duplicate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.original.clone() else { return };
        let this = cx.entity().downgrade();
        let root = self.root.clone();
        dialogs::prompt_text(window, cx, "复制流程", "新流程 key", format!("{key}-copy"), "复制", move |new_key, _, cx| {
            workflow::duplicate(&root, &key, &new_key)?;
            let _ = this.update(cx, |_, cx| cx.emit(WorkflowEditorEvent::Duplicated(new_key.clone())));
            Ok(())
        });
    }

    /// 依赖的流程：先运行它们（同一次运行里每个只跑一次），登录态沿用到本流程所有步骤
    fn render_needs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let (root, mine, view) = (self.root.clone(), self.needs.clone(), cx.entity().downgrade());
        let own_key = self.original.clone();
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(format!("依赖的流程 · {}", self.needs.len())))
                    .child(div().text_xs().text_color(muted).child("运行本流程前先运行它们（如「登录」），登录态沿用到下面所有步骤；同一次运行里每个依赖只跑一次")),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .children(self.needs.iter().enumerate().map(|(i, n)| {
                        h_flex()
                            .id(("wf-need", i))
                            .gap_1()
                            .pl_2()
                            .rounded(cx.theme().radius)
                            .border_1()
                            .border_color(cx.theme().border)
                            .child(Icon::new(IconName::Network).xsmall().text_color(muted))
                            .child(div().text_sm().child(n.clone()))
                            .child(Button::new(("wf-need-rm", i)).ghost().xsmall().icon(IconName::Close).tooltip("去掉这个依赖").on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if i < this.needs.len() {
                                        this.needs.remove(i);
                                        this.error = None;
                                        cx.notify();
                                    }
                                },
                            )))
                    }))
                    .child(Button::new("wf-need-add").xsmall().ghost().icon(IconName::Plus).label("添加依赖").dropdown_menu(move |menu, _, _| {
                        let flows: Vec<String> = workflow::list(&root)
                            .map(|l| l.into_iter().map(|(k, _)| k).filter(|k| Some(k.as_str()) != own_key.as_deref() && !mine.contains(k)).collect())
                            .unwrap_or_default();
                        if flows.is_empty() {
                            return menu.item(PopupMenuItem::new("没有可依赖的其他流程：先新建一个（如「登录」）").disabled(true));
                        }
                        flows.into_iter().fold(menu, |menu, k| {
                            let view = view.clone();
                            let key = k.clone();
                            menu.item(PopupMenuItem::new(k).icon(IconName::Network).on_click(move |_, _, cx| {
                                let key = key.clone();
                                let _ = view.update(cx, |this, cx| {
                                    this.needs.push(key);
                                    this.error = None;
                                    cx.notify();
                                });
                            }))
                        })
                    })),
            )
    }

    fn render_step(&self, ix: usize, s: &StepRow, total: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let id = s.id;
        let field = |label: &'static str, child: gpui_kit::AnyElement| {
            v_flex().gap_1().child(div().text_xs().text_color(muted).child(label)).child(child)
        };
        h_flex()
            .id(("wf-step", id))
            .gap_3()
            .p_3()
            .items_start()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .mt_5()
                    .w(px(24.))
                    .h(px(24.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(cx.theme().muted)
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("{}", ix + 1)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .child(field("运行目标（目录或用例）", Select::new(&s.run).small().placeholder("选择目录或用例").into_any_element()))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().child(field("步骤名", Input::new(&s.name).small().into_any_element())))
                            .child(div().flex_1().child(field("只跑这些标签", Input::new(&s.tags).small().into_any_element())))
                            .child(div().flex_1().child(field("只跑名称包含", Input::new(&s.name_filter).small().into_any_element())))
                            .child(div().w(px(120.)).child(field("用户（切换登录身份）", Input::new(&s.user).small().into_any_element()))),
                    ),
            )
            .child(
                v_flex()
                    .mt_5()
                    .gap_1()
                    .child(
                        Button::new(("wf-up", id))
                            .ghost()
                            .xsmall()
                            .icon(IconName::ArrowUp)
                            .tooltip("上移")
                            .disabled(ix == 0)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(i) = this.steps.iter().position(|s| s.id == id).filter(|i| *i > 0) {
                                    this.steps.swap(i, i - 1);
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        Button::new(("wf-down", id))
                            .ghost()
                            .xsmall()
                            .icon(IconName::ArrowDown)
                            .tooltip("下移")
                            .disabled(ix + 1 >= total)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(i) = this.steps.iter().position(|s| s.id == id).filter(|i| i + 1 < this.steps.len()) {
                                    this.steps.swap(i, i + 1);
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        Button::new(("wf-del", id))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Delete)
                            .tooltip("删除步骤")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.steps.retain(|s| s.id != id);
                                cx.notify();
                            })),
                    ),
            )
    }
}

impl Render for WorkflowEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let dirty = self.is_dirty(cx);
        let (key, wf) = self.collect(cx);
        let title = if wf.name.is_empty() { key.clone() } else { format!("{} · {key}", wf.name) };
        let command = format!("requestbaby workflow {key}");
        let total = self.steps.len();
        let is_new = self.original.is_none();
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .px_4()
                    .py_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(Icon::new(IconName::Network).small().text_color(cx.theme().primary))
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(if is_new { "新建流程".to_string() } else { title }))
                    .when(!is_new, |this| this.child(crate::style::cli_button("wf-cli", command.clone())))
                    .when(dirty && !is_new, |this| this.child(Tag::warning().outline().xsmall().child("未保存")))
                    .child(div().flex_1())
                    .child(
                        Button::new("wf-dry-run")
                            .small()
                            .outline()
                            .icon(IconName::Eye)
                            .label("Dry-run")
                            .tooltip("只编译不发送（先保存）")
                            .disabled(total == 0 && wf.needs.is_empty())
                            .on_click(cx.listener(|this, _, _, cx| this.run(true, cx))),
                    )
                    .child(
                        Button::new("wf-run")
                            .small()
                            .outline()
                            .icon(IconName::Play)
                            .label("运行")
                            .tooltip("保存并运行（requestbaby workflow <key>）")
                            .disabled(total == 0 && wf.needs.is_empty())
                            .on_click(cx.listener(|this, _, _, cx| this.run(false, cx))),
                    )
                    .child(
                        Button::new("wf-save")
                            .small()
                            .primary()
                            .icon(IconName::Check)
                            .label(if is_new { "创建" } else { "保存" })
                            .disabled(!dirty)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.save(cx);
                            })),
                    )
                    .child({
                        let view = cx.entity().downgrade();
                        Button::new("wf-more").small().ghost().icon(IconName::Ellipsis).dropdown_menu(move |menu, _, _| {
                            let (dup, del) = (view.clone(), view.clone());
                            menu.item(PopupMenuItem::new("复制为新流程…").icon(IconName::Copy).disabled(is_new).on_click(move |_, window, cx| {
                                let _ = dup.update(cx, |this, cx| this.duplicate(window, cx));
                            }))
                            .separator()
                            .item(PopupMenuItem::new(if is_new { "放弃新建" } else { "删除流程…" }).icon(IconName::Delete).on_click(
                                move |_, window, cx| {
                                    let _ = del.update(cx, |this, cx| this.delete(window, cx));
                                },
                            ))
                        })
                    }),
            )
            .child(
                v_flex()
                    .id("workflow-editor")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_4()
                    .gap_4()
                    .when_some(self.error.clone(), |this, e| {
                        this.child(
                            h_flex()
                                .gap_2()
                                .p_2()
                                .rounded(cx.theme().radius)
                                .bg(cx.theme().danger.opacity(0.1))
                                .text_xs()
                                .text_color(cx.theme().danger)
                                .child(Icon::new(IconName::TriangleAlert).xsmall())
                                .child(e),
                        )
                    })
                    .child(
                        h_flex()
                            .gap_3()
                            .child(
                                v_flex()
                                    .w(px(240.))
                                    .gap_1()
                                    .child(div().text_xs().text_color(muted).child("Key（命令行用）"))
                                    .child(Input::new(&self.key).small()),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .gap_1()
                                    .child(div().text_xs().text_color(muted).child("显示名"))
                                    .child(Input::new(&self.name).small()),
                            )
                            .child(
                                v_flex()
                                    .w(px(200.))
                                    .gap_1()
                                    .child(div().text_xs().text_color(muted).child("运行环境（固定后 verify 也在该环境运行它）"))
                                    .child(Select::new(&self.env).small()),
                            ),
                    )

                    .child(self.render_needs(cx))
                    .child(
                        v_flex()
                            .gap_2()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(format!("步骤 · {total}")))
                                    .child(div().text_xs().text_color(muted).child("按顺序执行；前序步骤捕获的变量自动传给后续步骤；某一步失败即中止")),
                            )
                            .when(total == 0, |this| {
                                this.child(
                                    div()
                                        .p_4()
                                        .rounded(cx.theme().radius)
                                        .border_1()
                                        .border_dashed()
                                        .border_color(cx.theme().border)
                                        .text_xs()
                                        .text_color(muted)
                                        .child("还没有步骤：添加一个目录（整个集合）或单个用例"),
                                )
                            })
                            .children(self.steps.iter().enumerate().map(|(ix, s)| self.render_step(ix, s, total, cx)).collect::<Vec<_>>())
                            .child(
                                h_flex().child(
                                    Button::new("wf-add-step")
                                        .small()
                                        .ghost()
                                        .icon(IconName::Plus)
                                        .label("添加步骤")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            let run = this.targets.first().map(|t| t.to_string()).unwrap_or_else(|| "collections".into());
                                            this.push_step(&WorkflowStep { run, ..Default::default() }, None, window, cx);
                                            cx.notify();
                                        })),
                                ),
                            ),
                    ),
            )
    }
}
