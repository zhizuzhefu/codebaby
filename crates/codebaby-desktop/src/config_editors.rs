//! 配置编辑器：collection.json / envs/*.json / codebaby.json。
//!
//! 与用例编辑器同一原则：文件是唯一事实；保存走 core 的规范化写出，随后立即校验。
use crate::kv_table::{KvMode, KvTable, KvTableEvent};
use codebaby_core::varstore::VarScope;
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
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div,
    prelude::FluentBuilder as _, px,
};
use codebaby_core::{fmt, proxy_setting, validate::ValidationIssue};
use serde_json::{Map, Value, json};
use std::path::PathBuf;

pub enum ConfigEditorEvent {
    Saved,
    /// 打开变量面板编辑该层变量（变量只有这一个编辑入口）
    EditVars(VarScope),
    /// 打开流程编辑器（None = 新建；流程只有这一个编辑入口）
    OpenWorkflow(Option<String>),
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
    /// 后台校验任务（新的一次替换旧的，旧结果随之作废）
    _validate: Option<Task<()>>,
}

impl FileState {
    fn load(path: PathBuf, rel: String, canonical: fn(&Value) -> String) -> Self {
        // 文件尚不存在（如目录还没有 collection.json）：按空文档编辑，保存时创建
        let text = std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".into());
        let (base, error) = match serde_json::from_str::<Value>(&text) {
            Ok(v) => (v, None),
            Err(e) => (json!({}), Some(format!("JSON 解析失败（请在文本编辑器中修复）: {e}"))),
        };
        Self { saved_text: canonical(&base), path, rel, base, dirty: false, error, issues: vec![], _validate: None }
    }

    /// 写入前以磁盘上的 vars 为准：变量由变量面板维护，这里不覆盖其最新修改
    fn with_disk_vars(&self, doc: Value) -> Value {
        self.with_disk_fields(doc, &["vars"])
    }

    /// 由其他专用编辑器维护的字段（vars → 变量面板，workflows → 流程编辑器）以磁盘为准
    fn with_disk_fields(&self, mut doc: Value, fields: &[&str]) -> Value {
        let disk = std::fs::read_to_string(&self.path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
        if let Value::Object(o) = &mut doc {
            for field in fields {
                match disk.as_ref().and_then(|d| d.get(*field)).cloned() {
                    Some(v) => {
                        o.insert((*field).into(), v);
                    }
                    None => {
                        o.remove(*field);
                    }
                }
            }
        }
        doc
    }

    fn save(&mut self, doc: Value, text: String) -> bool {
        if let Err(e) = codebaby_core::fsx::write(&self.path, &text) {
            self.error = Some(format!("写入失败: {e}"));
            return false;
        }
        self.base = doc;
        self.saved_text = text;
        self.dirty = false;
        self.error = None;
        true
    }
}

trait HasFile: Sized + 'static {
    fn file_mut(&mut self) -> &mut FileState;
}

/// 后台校验（全项目上下文，项目大时不卡界面）
fn revalidate<T: HasFile>(this: &mut T, env: String, cx: &mut Context<T>) {
    let path = this.file_mut().path.clone();
    let task = cx.background_spawn(async move { codebaby_core::validate::validate_file(&path, &env).unwrap_or_default() });
    this.file_mut()._validate = Some(cx.spawn(async move |view, cx| {
        let issues = task.await;
        let _ = view.update(cx, |t, cx| {
            t.file_mut().issues = issues;
            cx.notify();
        });
    }));
}

impl HasFile for CollectionEditor {
    fn file_mut(&mut self) -> &mut FileState {
        &mut self.file
    }
}

impl HasFile for ProjectEditor {
    fn file_mut(&mut self) -> &mut FileState {
        &mut self.file
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
    let warns = f.issues.iter().filter(|i| i.level != "error" && !i.is_style()).count();
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
        .children(f.issues.iter().filter(|i| !i.is_style()).map(|i| {
            let color = if i.level == "error" { cx.theme().danger } else { cx.theme().warning };
            h_flex()
                .gap_2()
                .text_xs()
                .child(Icon::new(IconName::TriangleAlert).xsmall().text_color(color))
                .child(div().text_color(cx.theme().muted_foreground).child(format!("[{}]", i.rule)))
                .child(i.message.clone())
        }))
}

/// 变量只读摘要 + 唯一编辑入口（变量面板）
fn vars_summary(title: &'static str, hint: &'static str, f: &FileState, scope: VarScope, cx: &mut Context<impl EventEmitter<ConfigEditorEvent>>) -> impl IntoElement {
    let names: Vec<String> = f.base.get("vars").and_then(|v| v.as_object()).map(|m| m.keys().cloned().collect()).unwrap_or_default();
    let muted = cx.theme().muted_foreground;
    v_flex()
        .gap_2()
        .child(
            h_flex()
                .gap_2()
                .child(section(title, hint, cx))
                .child(div().flex_1())
                .child(
                    Button::new("edit-vars")
                        .small()
                        .outline()
                        .icon(IconName::Settings2)
                        .label("在变量面板中编辑 →")
                        .on_click(cx.listener(move |_, _, _, cx| cx.emit(ConfigEditorEvent::EditVars(scope.clone())))),
                ),
        )
        .child(div().text_xs().text_color(muted).child(if names.is_empty() {
            "（未定义变量）".to_string()
        } else {
            format!("{} 个：{}", names.len(), names.join("、"))
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
    headers: Entity<KvTable>,
    setup: Entity<EditorState>,
    teardown: Entity<EditorState>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ConfigEditorEvent> for CollectionEditor {}

impl CollectionEditor {
    pub fn new(path: PathBuf, rel: String, env: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let file = FileState::load(path, rel, fmt::canonical_collection_text);
        let d = file.base.clone();
        let s = |k: &str| d.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let tags = d
            .get("tags")
            .and_then(|t| t.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "))
            .unwrap_or_default();
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("缺省为目录名").default_value(s("name")));
        let tags = cx.new(|cx| InputState::new(window, cx).placeholder("smoke, auth（逗号分隔，附加到集合内全部用例）").default_value(tags));
        let headers = cx.new(|_| KvTable::new(KvMode::Toggle, "请求头名"));
        headers.update(cx, |e, cx| e.load(d.get("headers").and_then(|h| h.as_object()), window, cx));
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
        subs.push(cx.subscribe(&headers, |this, _, _: &KvTableEvent, cx| this.refresh_dirty(cx)));
        let mut this = Self { file, env: env.clone(), name, tags, headers, setup, teardown, _subscriptions: subs };
        revalidate(&mut this, env, cx);
        this
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
        let doc = self.file.with_disk_vars(self.collect(cx));
        let text = fmt::canonical_collection_text(&doc);
        let ok = self.file.save(doc, text);
        if ok {
            revalidate(self, self.env.clone(), cx);
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
                    .child(v_flex().gap_2().child(section("请求头", "逐级继承，用例同名覆盖；值支持 {{var}}；取消勾选 = 保留但不发送", cx)).child(self.headers.clone()))
                    .child(vars_summary(
                        "集合变量",
                        "仅对本集合内用例生效；覆盖环境与项目变量",
                        &self.file,
                        VarScope::Collection(self.file.rel.trim_end_matches("/collection.json").to_string()),
                        cx,
                    ))
                    .child(v_flex().gap_1().child(section("setup", "进入集合时执行一次", cx)).child(Editor::new(&self.setup).h(px(96.))))
                    .child(v_flex().gap_1().child(section("teardown", "离开集合时执行一次", cx)).child(Editor::new(&self.teardown).h(px(96.))))
                    .child(render_problems(&self.file, cx)),
            )
    }
}

// ------------------------------------------------------------------ codebaby.json

pub struct ProjectEditor {
    file: FileState,
    env: String,
    default_env: Entity<SelectState<Vec<SharedString>>>,
    timeout: Entity<InputState>,
    /// 代理：跟随系统 / 使用代理 / 不使用代理（与 CLI `codebaby proxy` 同一读写）
    proxy_mode: Entity<SelectState<Vec<SharedString>>>,
    proxy_url: Entity<InputState>,
    proxy_bypass: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

const PROXY_MODES: [proxy_setting::Mode; 3] = [proxy_setting::Mode::System, proxy_setting::Mode::On, proxy_setting::Mode::Off];

impl EventEmitter<ConfigEditorEvent> for ProjectEditor {}

impl ProjectEditor {
    pub fn new(
        path: PathBuf,
        rel: String,
        env: String,
        envs: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let file = FileState::load(path, rel, fmt::canonical_project_text);
        let d = file.base.clone();
        let default = d.get("defaultEnv").and_then(|v| v.as_str()).unwrap_or("dev").to_string();
        let env_items: Vec<SharedString> = envs.iter().map(|e| SharedString::from(e.clone())).collect();
        let env_ix = envs.iter().position(|e| *e == default).map(IndexPath::new);
        let default_env = cx.new(|cx| SelectState::new(env_items, env_ix, window, cx));
        let timeout_text = d.get("timeoutMs").map(|v| v.to_string()).unwrap_or_default();
        let timeout = cx.new(|cx| InputState::new(window, cx).placeholder("30000").default_value(timeout_text));
        let proxy: Option<codebaby_core::config::ProxyConfig> = d.get("proxy").and_then(|v| serde_json::from_value(v.clone()).ok());
        let mode = proxy_setting::Mode::of(proxy.as_ref());
        let mode_items: Vec<SharedString> = PROXY_MODES.iter().map(|m| SharedString::from(m.label())).collect();
        let mode_ix = PROXY_MODES.iter().position(|m| *m == mode).map(IndexPath::new);
        let proxy_mode = cx.new(|cx| SelectState::new(mode_items, mode_ix, window, cx));
        let url_text = proxy.as_ref().map(|p| p.url.clone()).unwrap_or_default();
        let proxy_url = cx.new(|cx| InputState::new(window, cx).placeholder("socks5://127.0.0.1:1080").default_value(url_text));
        let bypass_text = proxy.as_ref().map(|p| p.no_proxy.join(", ")).unwrap_or_default();
        let proxy_bypass = cx.new(|cx| InputState::new(window, cx).placeholder("例：corp.internal, 10.0.0.0/8").default_value(bypass_text));

        let mut this = Self {
            file,
            env,
            default_env,
            timeout,
            proxy_mode,
            proxy_url,
            proxy_bypass,
            _subscriptions: vec![],
        };
        let mut subs = vec![];
        subscribe_input(&mut subs, &this.timeout, window, cx, Self::refresh_dirty);
        subscribe_input(&mut subs, &this.proxy_url, window, cx, Self::refresh_dirty);
        subscribe_input(&mut subs, &this.proxy_bypass, window, cx, Self::refresh_dirty);
        subs.push(cx.subscribe_in(&this.proxy_mode, window, |this, _, _: &SelectEvent<Vec<SharedString>>, _, cx| {
            this.refresh_dirty(cx)
        }));
        subs.push(cx.subscribe_in(&this.default_env, window, |this, _, _: &SelectEvent<Vec<SharedString>>, _, cx| {
            this.refresh_dirty(cx)
        }));
        this._subscriptions = subs;
        let env = this.env.clone();
        revalidate(&mut this, env, cx);
        this
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
        // 代理：跟随系统 = 不写；其余写 enabled / url / noProxy（地址不合法时照写，由校验指出）
        match self.proxy_mode_of(cx) {
            proxy_setting::Mode::System => {
                d.remove("proxy");
            }
            mode => {
                let cfg = codebaby_core::config::ProxyConfig {
                    enabled: mode == proxy_setting::Mode::On,
                    url: self.proxy_url.read(cx).value().trim().to_string(),
                    no_proxy: proxy_setting::parse_no_proxy(&self.proxy_bypass.read(cx).value()),
                };
                d.insert("proxy".into(), serde_json::to_value(cfg).unwrap_or_default());
            }
        }
        Value::Object(d)
    }

    fn proxy_mode_of(&self, cx: &App) -> proxy_setting::Mode {
        let label = self.proxy_mode.read(cx).selected_value().map(|v| v.to_string()).unwrap_or_default();
        PROXY_MODES.into_iter().find(|m| m.label() == label).unwrap_or(proxy_setting::Mode::System)
    }

    fn refresh_dirty(&mut self, cx: &mut Context<Self>) {
        let dirty = fmt::canonical_project_text(&self.collect(cx)) != self.file.saved_text;
        if dirty != self.file.dirty {
            self.file.dirty = dirty;
            cx.notify();
        }
    }

    pub fn save(&mut self, cx: &mut Context<Self>) -> bool {
        let doc = self.file.with_disk_fields(self.collect(cx), &["vars", "workflows"]);
        let text = fmt::canonical_project_text(&doc);
        let ok = self.file.save(doc, text);
        if ok {
            revalidate(self, self.env.clone(), cx);
            cx.emit(ConfigEditorEvent::Saved);
        }
        cx.notify();
        ok
    }

    /// 代理：模式 + 地址 + 直连主机；说明当前实际生效的规则
    fn render_proxy(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let mode = self.proxy_mode_of(cx);
        let note = match mode {
            proxy_setting::Mode::System => "沿用环境变量和系统的代理设置，都没有则直连",
            proxy_setting::Mode::On => "所有请求、浏览器、adb 都经这个代理；localhost 始终直连",
            proxy_setting::Mode::Off => "全部直连，忽略环境变量和系统的代理；地址会保留",
        };
        v_flex()
            .gap_2()
            .child(section("代理", "所有对外连接走同一个代理", cx))
            .child(
                h_flex()
                    .gap_3()
                    .items_end()
                    .child(v_flex().w(px(200.)).gap_1().child(div().text_xs().text_color(muted).child("代理模式")).child(Select::new(&self.proxy_mode).small()))
                    .when(mode != proxy_setting::Mode::System, |this| {
                        this.child(v_flex().flex_1().gap_1().child(div().text_xs().text_color(muted).child("代理地址（socks5 / http）")).child(Input::new(&self.proxy_url).small()))
                            .child(v_flex().w(px(260.)).gap_1().child(div().text_xs().text_color(muted).child("直连的主机（逗号分隔）")).child(Input::new(&self.proxy_bypass).small()))
                    }),
            )
            .child(div().text_xs().text_color(muted).child(note))
    }

    /// 流程概览：编辑在独立的流程编辑器中（与 CLI `codebaby workflow …` 同一实现）
    fn render_workflows(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let workflows = codebaby_core::workflow::list(self.file.path.parent().unwrap_or(std::path::Path::new("."))).unwrap_or_default();
        v_flex()
            .gap_1()
            .when(workflows.is_empty(), |this| this.child(div().text_xs().text_color(muted).child("还没有流程")))
            .children(workflows.into_iter().enumerate().map(|(ix, (key, wf))| {
                let label = if wf.name.is_empty() { key.clone() } else { format!("{} · {key}", wf.name) };
                let open_key = key.clone();
                h_flex()
                    .id(("project-wf", ix))
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded(cx.theme().radius)
                    .cursor_pointer()
                    .hover(|d| d.bg(cx.theme().muted))
                    .child(Icon::new(IconName::Network).xsmall().text_color(muted))
                    .child(div().flex_1().text_sm().child(label))
                    .child(div().text_xs().text_color(muted).child(format!("{} 步", wf.steps.len())))
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(ConfigEditorEvent::OpenWorkflow(Some(open_key.clone())))))
            }))
            .child(
                h_flex().child(
                    Button::new("project-new-wf")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Plus)
                        .label("新建流程")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(ConfigEditorEvent::OpenWorkflow(None)))),
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
                    .child(self.render_proxy(cx))
                    .child(vars_summary("项目变量", "所有环境共享，优先级最低：环境 / 集合 / 运行时同名变量依次覆盖", &self.file, VarScope::Project, cx))
                    .child(
                        v_flex()
                            .gap_2()
                            .child(section("流程", "多阶段编排：步骤共享运行时变量，任一步骤失败即中止", cx))
                            .child(self.render_workflows(cx)),
                    )
                    .child(render_problems(&self.file, cx)),
            )
    }
}
