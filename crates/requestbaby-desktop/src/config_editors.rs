//! 配置编辑器：collection.json / envs/*.json / requestbaby.json。
//!
//! 与用例编辑器同一原则：文件是唯一事实；保存走 core 的规范化写出，随后立即校验。
use crate::vars_editor::{VarsEditor, VarsEditorEvent};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, IndexPath, Sizable as _, h_flex,
    button::{Button, ButtonVariants as _},
    input::{Editor, EditorState, Input, InputEvent, InputState},
    select::{Select, SelectEvent, SelectState},
    tag::Tag,
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
};
use requestbaby_core::{fmt, validate::ValidationIssue};
use serde_json::{Map, Value, json};
use std::path::PathBuf;

pub enum ConfigEditorEvent {
    Saved,
    DuplicateEnv(String),
    DeleteEnv(String),
}

/// 三种编辑器共享的文件状态
struct FileState {
    path: PathBuf,
    rel: String,
    base: Value,
    saved_text: String,
    dirty: bool,
    error: Option<String>,
    issues: Vec<ValidationIssue>,
}

impl FileState {
    fn load(path: PathBuf, rel: String, canonical: fn(&Value) -> String) -> Self {
        // 文件尚不存在（如目录还没有 collection.json）：按空文档编辑，保存时创建
        let text = std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".into());
        let (base, error) = match serde_json::from_str::<Value>(&text) {
            Ok(v) => (v, None),
            Err(e) => (json!({}), Some(format!("JSON 解析失败（请在文本编辑器中修复）: {e}"))),
        };
        Self { saved_text: canonical(&base), path, rel, base, dirty: false, error, issues: vec![] }
    }

    fn save(&mut self, doc: Value, text: String, env: &str) -> bool {
        if let Err(e) = std::fs::write(&self.path, &text) {
            self.error = Some(format!("写入失败: {e}"));
            return false;
        }
        self.base = doc;
        self.saved_text = text;
        self.dirty = false;
        self.error = None;
        self.validate(env);
        true
    }

    fn validate(&mut self, env: &str) {
        self.issues = requestbaby_core::validate::validate_file(&self.path, env).unwrap_or_default();
    }
}

fn subscribe_input<T: 'static>(
    subs: &mut Vec<Subscription>,
    input: &Entity<InputState>,
    window: &mut Window,
    cx: &mut Context<T>,
    on_change: fn(&mut T, &mut Context<T>),
) {
    subs.push(cx.subscribe_in(input, window, move |this, _, ev: &InputEvent, _, cx| {
        if matches!(ev, InputEvent::Change) {
            on_change(this, cx);
        }
    }));
}

fn js_editor(placeholder: &str, window: &mut Window, cx: &mut Context<EditorState>) -> EditorState {
    EditorState::new(window, cx).language("javascript".to_string()).line_number(true).soft_wrap(true).placeholder(placeholder.to_string())
}

fn section(title: &'static str, hint: &'static str, cx: &App) -> impl IntoElement {
    h_flex()
        .gap_2()
        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(title))
        .child(div().text_xs().text_color(cx.theme().muted_foreground).child(hint))
}

fn render_header(
    title: String,
    icon: IconName,
    f: &FileState,
    save: Button,
    extra: Vec<AnyElement>,
    cx: &App,
) -> impl IntoElement {
    let errors = f.issues.iter().filter(|i| i.level == "error").count();
    let warns = f.issues.len() - errors;
    h_flex()
        .px_4()
        .py_2()
        .gap_3()
        .border_b_1()
        .border_color(cx.theme().border)
        .child(Icon::new(icon).small().text_color(cx.theme().muted_foreground))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    h_flex()
                        .gap_2()
                        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(title))
                        .when(f.dirty, |this| this.child(Tag::warning().outline().xsmall().child("未保存"))),
                )
                .child(div().text_xs().text_color(cx.theme().muted_foreground).child(f.rel.clone())),
        )
        .child(
            h_flex()
                .gap_2()
                .when(errors > 0, |this| this.child(Tag::danger().outline().xsmall().child(format!("{errors} 个错误"))))
                .when(warns > 0, |this| this.child(Tag::warning().outline().xsmall().child(format!("{warns} 个警告"))))
                .when(errors == 0 && warns == 0 && f.error.is_none(), |this| {
                    this.child(Tag::success().outline().xsmall().child("校验通过"))
                })
                .children(extra)
                .child(save),
        )
}

fn render_problems(f: &FileState, cx: &App) -> impl IntoElement {
    v_flex()
        .gap_1()
        .when_some(f.error.clone(), |this, e| this.child(div().text_xs().text_color(cx.theme().danger).child(e)))
        .children(f.issues.iter().map(|i| {
            let color = if i.level == "error" { cx.theme().danger } else { cx.theme().warning };
            h_flex()
                .gap_2()
                .text_xs()
                .child(Icon::new(IconName::TriangleAlert).xsmall().text_color(color))
                .child(div().text_color(cx.theme().muted_foreground).child(format!("[{}]", i.rule)))
                .child(i.message.clone())
        }))
}

fn save_button(id: &'static str, dirty: bool) -> Button {
    Button::new(id).small().primary().icon(IconName::Check).label("保存").disabled(!dirty)
}

// ------------------------------------------------------------------ collection.json

pub struct CollectionEditor {
    file: FileState,
    env: String,
    name: Entity<InputState>,
    tags: Entity<InputState>,
    headers: Entity<VarsEditor>,
    vars: Entity<VarsEditor>,
    setup: Entity<EditorState>,
    teardown: Entity<EditorState>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ConfigEditorEvent> for CollectionEditor {}

impl CollectionEditor {
    pub fn new(path: PathBuf, rel: String, env: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut file = FileState::load(path, rel, fmt::canonical_collection_text);
        file.validate(&env);
        let d = file.base.clone();
        let s = |k: &str| d.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let tags = d
            .get("tags")
            .and_then(|t| t.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "))
            .unwrap_or_default();
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("缺省为目录名").default_value(s("name")));
        let tags = cx.new(|cx| InputState::new(window, cx).placeholder("smoke, auth（逗号分隔，附加到集合内全部用例）").default_value(tags));
        let headers = cx.new(|_| VarsEditor::new(false));
        headers.update(cx, |e, cx| e.load(d.get("headers").and_then(|h| h.as_object()), window, cx));
        let vars = cx.new(|_| VarsEditor::new(true));
        vars.update(cx, |e, cx| e.load(d.get("vars").and_then(|h| h.as_object()), window, cx));
        let setup_text = s("setup");
        let teardown_text = s("teardown");
        let setup = cx.new(|cx| js_editor("进入集合前执行：可写 vars、可 http.get/post 造数", window, cx).default_value(setup_text));
        let teardown = cx.new(|cx| js_editor("离开集合后执行（无论成败）：清理数据", window, cx).default_value(teardown_text));

        let mut subs = vec![];
        subscribe_input(&mut subs, &name, window, cx, Self::refresh_dirty);
        subscribe_input(&mut subs, &tags, window, cx, Self::refresh_dirty);
        for e in [&setup, &teardown] {
            subs.push(cx.subscribe_in(e, window, |this, _, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.refresh_dirty(cx);
                }
            }));
        }
        for v in [&headers, &vars] {
            subs.push(cx.subscribe(v, |this, _, _: &VarsEditorEvent, cx| this.refresh_dirty(cx)));
        }
        Self { file, env, name, tags, headers, vars, setup, teardown, _subscriptions: subs }
    }

    pub fn path(&self) -> &PathBuf {
        &self.file.path
    }

    pub fn is_dirty(&self) -> bool {
        self.file.dirty
    }

    fn collect(&self, cx: &App) -> Value {
        let mut d = match &self.file.base {
            Value::Object(o) => o.clone(),
            _ => Map::new(),
        };
        let text = |e: &Entity<EditorState>| e.read(cx).value().trim_end().to_string();
        d.insert("name".into(), Value::String(self.name.read(cx).value().trim().to_string()));
        let tags: Vec<Value> = self
            .tags
            .read(cx)
            .value()
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| Value::String(t.to_string()))
            .collect();
        d.insert("tags".into(), Value::Array(tags));
        d.insert("headers".into(), Value::Object(self.headers.read(cx).to_map(cx)));
        d.insert("vars".into(), Value::Object(self.vars.read(cx).to_map(cx)));
        d.insert("setup".into(), Value::String(text(&self.setup)));
        d.insert("teardown".into(), Value::String(text(&self.teardown)));
        Value::Object(d)
    }

    fn refresh_dirty(&mut self, cx: &mut Context<Self>) {
        let dirty = fmt::canonical_collection_text(&self.collect(cx)) != self.file.saved_text;
        if dirty != self.file.dirty {
            self.file.dirty = dirty;
            cx.notify();
        }
    }

    pub fn save(&mut self, cx: &mut Context<Self>) -> bool {
        let doc = self.collect(cx);
        let text = fmt::canonical_collection_text(&doc);
        let ok = self.file.save(doc, text, &self.env.clone());
        if ok {
            cx.emit(ConfigEditorEvent::Saved);
        }
        cx.notify();
        ok
    }
}

impl Render for CollectionEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let save = save_button("save-collection", self.file.dirty).on_click(cx.listener(|this, _, _, cx| {
            this.save(cx);
        }));
        let title = {
            let n = self.name.read(cx).value().to_string();
            if n.is_empty() { "集合配置".to_string() } else { format!("集合 · {n}") }
        };
        v_flex()
            .size_full()
            .child(render_header(title, IconName::FolderOpen, &self.file, save, vec![], cx))
            .child(
                v_flex()
                    .id("collection-editor")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_4()
                    .gap_4()
                    .child(
                        h_flex()
                            .gap_3()
                            .child(v_flex().flex_1().gap_1().child(section("名称", "", cx)).child(Input::new(&self.name).small()))
                            .child(v_flex().flex_1().gap_1().child(section("标签", "", cx)).child(Input::new(&self.tags).small())),
                    )
                    .child(v_flex().gap_2().child(section("请求头", "逐级继承，用例同名覆盖；值支持 {{var}}", cx)).child(self.headers.clone()))
                    .child(v_flex().gap_2().child(section("集合变量", "覆盖环境变量，被运行时捕获覆盖", cx)).child(self.vars.clone()))
                    .child(v_flex().gap_1().child(section("setup", "进入集合时执行一次", cx)).child(Editor::new(&self.setup).h(px(96.))))
                    .child(v_flex().gap_1().child(section("teardown", "离开集合时执行一次", cx)).child(Editor::new(&self.teardown).h(px(96.))))
                    .child(render_problems(&self.file, cx)),
            )
    }
}

// ------------------------------------------------------------------ envs/<name>.json

pub struct EnvEditor {
    file: FileState,
    name: String,
    is_default: bool,
    vars: Entity<VarsEditor>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ConfigEditorEvent> for EnvEditor {}

impl EnvEditor {
    pub fn new(path: PathBuf, rel: String, name: String, is_default: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut file = FileState::load(path, rel, fmt::canonical_env_text);
        file.validate(&name);
        let vars = cx.new(|_| VarsEditor::new(true));
        vars.update(cx, |e, cx| e.load(file.base.get("vars").and_then(|v| v.as_object()), window, cx));
        let subs = vec![cx.subscribe(&vars, |this: &mut Self, _, _: &VarsEditorEvent, cx| {
            let dirty = fmt::canonical_env_text(&this.collect(cx)) != this.file.saved_text;
            if dirty != this.file.dirty {
                this.file.dirty = dirty;
                cx.notify();
            }
        })];
        Self { file, name, is_default, vars, _subscriptions: subs }
    }

    pub fn path(&self) -> &PathBuf {
        &self.file.path
    }

    pub fn is_dirty(&self) -> bool {
        self.file.dirty
    }

    fn collect(&self, cx: &App) -> Value {
        let mut d = match &self.file.base {
            Value::Object(o) => o.clone(),
            _ => Map::new(),
        };
        d.insert("vars".into(), Value::Object(self.vars.read(cx).to_map(cx)));
        Value::Object(d)
    }

    pub fn save(&mut self, cx: &mut Context<Self>) -> bool {
        let doc = self.collect(cx);
        let text = fmt::canonical_env_text(&doc);
        let env = self.name.clone();
        let ok = self.file.save(doc, text, &env);
        if ok {
            cx.emit(ConfigEditorEvent::Saved);
        }
        cx.notify();
        ok
    }
}

impl Render for EnvEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let save = save_button("save-env", self.file.dirty).on_click(cx.listener(|this, _, _, cx| {
            this.save(cx);
        }));
        let name = self.name.clone();
        let name2 = self.name.clone();
        let extra = vec![
            Button::new("dup-env")
                .small()
                .outline()
                .icon(IconName::Copy)
                .label("复制为新环境")
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(ConfigEditorEvent::DuplicateEnv(name.clone()))))
                .into_any_element(),
            Button::new("del-env")
                .small()
                .ghost()
                .icon(IconName::Delete)
                .disabled(self.is_default)
                .tooltip(if self.is_default { "默认环境不能删除" } else { "删除环境" })
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(ConfigEditorEvent::DeleteEnv(name2.clone()))))
                .into_any_element(),
        ];
        v_flex()
            .size_full()
            .child(render_header(
                format!("环境 · {}{}", self.name, if self.is_default { "（默认）" } else { "" }),
                IconName::Globe,
                &self.file,
                save,
                extra,
                cx,
            ))
            .child(
                v_flex()
                    .id("env-editor")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_4()
                    .gap_3()
                    .child(section("环境变量", "覆盖项目变量；Secret 值在 curl 命令、报告、日志中一律脱敏", cx))
                    .child(self.vars.clone())
                    .child(render_problems(&self.file, cx)),
            )
    }
}

// ------------------------------------------------------------------ requestbaby.json

struct StepRow {
    id: usize,
    name: Entity<InputState>,
    run: Entity<SelectState<Vec<SharedString>>>,
    tags: Entity<InputState>,
    name_filter: Entity<InputState>,
}

struct WorkflowRow {
    id: usize,
    key: Entity<InputState>,
    name: Entity<InputState>,
    steps: Vec<StepRow>,
}

pub struct ProjectEditor {
    file: FileState,
    env: String,
    default_env: Entity<SelectState<Vec<SharedString>>>,
    timeout: Entity<InputState>,
    vars: Entity<VarsEditor>,
    workflows: Vec<WorkflowRow>,
    /// 步骤 run 的候选：集合目录 + 用例文件（相对项目根）
    targets: Vec<SharedString>,
    next_id: usize,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ConfigEditorEvent> for ProjectEditor {}

impl ProjectEditor {
    pub fn new(
        path: PathBuf,
        rel: String,
        env: String,
        envs: Vec<String>,
        targets: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut file = FileState::load(path, rel, fmt::canonical_project_text);
        file.validate(&env);
        let d = file.base.clone();
        let default = d.get("defaultEnv").and_then(|v| v.as_str()).unwrap_or("dev").to_string();
        let env_items: Vec<SharedString> = envs.iter().map(|e| SharedString::from(e.clone())).collect();
        let env_ix = envs.iter().position(|e| *e == default).map(IndexPath::new);
        let default_env = cx.new(|cx| SelectState::new(env_items, env_ix, window, cx));
        let timeout_text = d.get("timeoutMs").map(|v| v.to_string()).unwrap_or_default();
        let timeout = cx.new(|cx| InputState::new(window, cx).placeholder("30000").default_value(timeout_text));
        let vars = cx.new(|_| VarsEditor::new(true));
        vars.update(cx, |e, cx| e.load(d.get("vars").and_then(|v| v.as_object()), window, cx));

        let mut this = Self {
            file,
            env,
            default_env,
            timeout,
            vars,
            workflows: vec![],
            targets: targets.into_iter().map(SharedString::from).collect(),
            next_id: 0,
            _subscriptions: vec![],
        };
        let mut subs = vec![];
        subscribe_input(&mut subs, &this.timeout, window, cx, Self::refresh_dirty);
        subs.push(cx.subscribe(&this.vars, |this, _, _: &VarsEditorEvent, cx| this.refresh_dirty(cx)));
        subs.push(cx.subscribe_in(&this.default_env, window, |this, _, _: &SelectEvent<Vec<SharedString>>, _, cx| {
            this.refresh_dirty(cx)
        }));
        this._subscriptions = subs;
        if let Some(wfs) = d.get("workflows").and_then(|w| w.as_object()) {
            for (key, wf) in wfs {
                this.push_workflow(key, wf, window, cx);
            }
        }
        this
    }

    pub fn path(&self) -> &PathBuf {
        &self.file.path
    }

    pub fn is_dirty(&self) -> bool {
        self.file.dirty
    }

    fn id(&mut self) -> usize {
        self.next_id += 1;
        self.next_id
    }

    fn input(&mut self, value: &str, placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        let value = value.to_string();
        let e = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(value));
        let mut subs = std::mem::take(&mut self._subscriptions);
        subscribe_input(&mut subs, &e, window, cx, Self::refresh_dirty);
        self._subscriptions = subs;
        e
    }

    fn push_step(&mut self, wf_ix: usize, step: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let s = |k: &str| step.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let tags = step
            .get("tags")
            .and_then(|t| t.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "))
            .unwrap_or_default();
        let id = self.id();
        let name = self.input(&s("name"), "步骤名（可选）", window, cx);
        let tags = self.input(&tags, "标签过滤", window, cx);
        let name_filter = self.input(&s("nameFilter"), "用例名过滤", window, cx);
        let current = SharedString::from(s("run"));
        let mut items = self.targets.clone();
        if !current.is_empty() && !items.contains(&current) {
            items.push(current.clone());
        }
        let ix = items.iter().position(|t| *t == current).map(IndexPath::new);
        let run = cx.new(|cx| SelectState::new(items, ix, window, cx).searchable(true));
        self._subscriptions.push(cx.subscribe_in(&run, window, |this, _, _: &SelectEvent<Vec<SharedString>>, _, cx| {
            this.refresh_dirty(cx)
        }));
        if let Some(wf) = self.workflows.get_mut(wf_ix) {
            wf.steps.push(StepRow { id, name, run, tags, name_filter });
        }
    }

    fn push_workflow(&mut self, key: &str, wf: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.id();
        let key = self.input(key, "工作流 key（CLI: requestbaby workflow <key>）", window, cx);
        let display = wf.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let name = self.input(&display, "显示名（可选）", window, cx);
        self.workflows.push(WorkflowRow { id, key, name, steps: vec![] });
        let ix = self.workflows.len() - 1;
        if let Some(steps) = wf.get("steps").and_then(|s| s.as_array()) {
            for step in steps {
                self.push_step(ix, step, window, cx);
            }
        }
    }

    fn collect(&self, cx: &App) -> Value {
        let mut d = match &self.file.base {
            Value::Object(o) => o.clone(),
            _ => Map::new(),
        };
        if let Some(env) = self.default_env.read(cx).selected_value() {
            d.insert("defaultEnv".into(), Value::String(env.to_string()));
        }
        let timeout = self.timeout.read(cx).value().trim().to_string();
        match timeout.parse::<u64>() {
            Ok(n) => {
                d.insert("timeoutMs".into(), json!(n));
            }
            Err(_) if timeout.is_empty() => {
                d.remove("timeoutMs");
            }
            Err(_) => {
                d.insert("timeoutMs".into(), Value::String(timeout));
            }
        }
        d.insert("vars".into(), Value::Object(self.vars.read(cx).to_map(cx)));
        let mut wfs = Map::new();
        for wf in &self.workflows {
            let key = wf.key.read(cx).value().trim().to_string();
            if key.is_empty() {
                continue;
            }
            let mut w = Map::new();
            let name = wf.name.read(cx).value().trim().to_string();
            if !name.is_empty() {
                w.insert("name".into(), Value::String(name));
            }
            let steps: Vec<Value> = wf
                .steps
                .iter()
                .map(|s| {
                    let mut o = Map::new();
                    let name = s.name.read(cx).value().trim().to_string();
                    if !name.is_empty() {
                        o.insert("name".into(), Value::String(name));
                    }
                    let run = s.run.read(cx).selected_value().map(|v| v.to_string()).unwrap_or_default();
                    o.insert("run".into(), Value::String(run));
                    let tags: Vec<Value> = s
                        .tags
                        .read(cx)
                        .value()
                        .split(',')
                        .map(str::trim)
                        .filter(|t| !t.is_empty())
                        .map(|t| Value::String(t.into()))
                        .collect();
                    if !tags.is_empty() {
                        o.insert("tags".into(), Value::Array(tags));
                    }
                    let nf = s.name_filter.read(cx).value().trim().to_string();
                    if !nf.is_empty() {
                        o.insert("nameFilter".into(), Value::String(nf));
                    }
                    Value::Object(o)
                })
                .collect();
            w.insert("steps".into(), Value::Array(steps));
            wfs.insert(key, Value::Object(w));
        }
        d.insert("workflows".into(), Value::Object(wfs));
        Value::Object(d)
    }

    fn refresh_dirty(&mut self, cx: &mut Context<Self>) {
        let dirty = fmt::canonical_project_text(&self.collect(cx)) != self.file.saved_text;
        if dirty != self.file.dirty {
            self.file.dirty = dirty;
            cx.notify();
        }
    }

    pub fn save(&mut self, cx: &mut Context<Self>) -> bool {
        let doc = self.collect(cx);
        let text = fmt::canonical_project_text(&doc);
        let env = self.env.clone();
        let ok = self.file.save(doc, text, &env);
        if ok {
            cx.emit(ConfigEditorEvent::Saved);
        }
        cx.notify();
        ok
    }

    fn render_workflows(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .gap_3()
            .children(self.workflows.iter().enumerate().map(|(wf_ix, wf)| {
                let wf_id = wf.id;
                v_flex()
                    .p_3()
                    .gap_2()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .gap_2()
                            .child(Icon::new(IconName::Network).small().text_color(muted))
                            .child(div().w(px(220.)).child(Input::new(&wf.key).small()))
                            .child(div().flex_1().child(Input::new(&wf.name).small()))
                            .child(
                                Button::new(("del-wf", wf_id))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Delete)
                                    .tooltip("删除工作流")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.workflows.retain(|w| w.id != wf_id);
                                        this.refresh_dirty(cx);
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .pl_6()
                            .text_xs()
                            .text_color(muted)
                            .child(div().w(px(24.)).child("#"))
                            .child(div().w(px(260.)).child("运行目标"))
                            .child(div().w(px(150.)).child("步骤名"))
                            .child(div().w(px(130.)).child("标签"))
                            .child(div().flex_1().child("用例名")),
                    )
                    .children(wf.steps.iter().enumerate().map(|(ix, s)| {
                        let step_id = s.id;
                        h_flex()
                            .gap_2()
                            .pl_6()
                            .child(div().w(px(24.)).text_xs().text_color(muted).child(format!("{}", ix + 1)))
                            .child(div().w(px(260.)).child(Select::new(&s.run).small().placeholder("选择目录或用例")))
                            .child(div().w(px(150.)).child(Input::new(&s.name).small()))
                            .child(div().w(px(130.)).child(Input::new(&s.tags).small()))
                            .child(div().flex_1().child(Input::new(&s.name_filter).small()))
                            .child(
                                Button::new(("del-step", step_id))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Close)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(w) = this.workflows.get_mut(wf_ix) {
                                            w.steps.retain(|s| s.id != step_id);
                                        }
                                        this.refresh_dirty(cx);
                                        cx.notify();
                                    })),
                            )
                    }))
                    .child(
                        h_flex().pl_6().child(
                            Button::new(("add-step", wf_id))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Plus)
                                .label("添加步骤")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.push_step(wf_ix, &json!({ "run": "collections" }), window, cx);
                                    this.refresh_dirty(cx);
                                    cx.notify();
                                })),
                        ),
                    )
            }))
            .child(
                h_flex().child(
                    Button::new("add-wf")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Plus)
                        .label("添加工作流")
                        .on_click(cx.listener(|this, _, window, cx| {
                            let n = this.workflows.len() + 1;
                            this.push_workflow(&format!("workflow{n}"), &json!({ "steps": [{ "run": "collections" }] }), window, cx);
                            this.refresh_dirty(cx);
                            cx.notify();
                        })),
                ),
            )
    }
}

impl Render for ProjectEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let save = save_button("save-project", self.file.dirty).on_click(cx.listener(|this, _, _, cx| {
            this.save(cx);
        }));
        v_flex()
            .size_full()
            .child(render_header("项目设置".into(), IconName::Settings, &self.file, save, vec![], cx))
            .child(
                v_flex()
                    .id("project-editor")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_4()
                    .gap_4()
                    .child(
                        h_flex()
                            .gap_3()
                            .child(v_flex().w(px(200.)).gap_1().child(section("默认环境", "", cx)).child(Select::new(&self.default_env).small()))
                            .child(v_flex().w(px(200.)).gap_1().child(section("默认超时 ms", "", cx)).child(Input::new(&self.timeout).small())),
                    )
                    .child(v_flex().gap_2().child(section("全局变量", "最低优先级：环境 / 集合 / 运行时依次覆盖", cx)).child(self.vars.clone()))
                    .child(
                        v_flex()
                            .gap_2()
                            .child(section("工作流", "多阶段编排：步骤共享运行时变量，任一步骤失败即中止", cx))
                            .child(self.render_workflows(cx)),
                    )
                    .child(render_problems(&self.file, cx)),
            )
    }
}
