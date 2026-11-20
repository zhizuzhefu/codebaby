//! 用例编辑器：请求 / 断言 / 脚本 / JSON 等视图编辑同一个用例文件。
//!
//! 界面与文件一一对应：「断言」页 = expect（状态码 / 响应体 / 响应头）+ capture；「脚本（进阶）」= tests / pre / post。
//!
//! 文件是唯一事实：表单只覆盖它认识的字段，其余字段（skip、cases、timeoutMs…）原样保留；
//! 保存走 core 的规范化格式（与 `requestbaby fmt` 字节一致），随后立即跑 `validate`。
use crate::style;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _, Icon, IconName, IndexPath, Sizable as _, h_flex,
    button::{Button, ButtonGroup, ButtonVariants as _},
    input::{Editor, EditorState, Input, InputEvent, InputState},
    radio::RadioGroup,
    switch::Switch,
    label::Label,
    select::{Select, SelectEvent, SelectState},
    tab::{Tab, TabBar},
    tag::Tag,
    v_flex,
};
use gpui_kit::{
    AnyElement,    App, AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div,
    prelude::FluentBuilder as _, px,
};
use crate::case_form::{BodyForm, CaseForm, GrpcForm, StepForm, UiForm, WsForm};
use crate::expect_table::{ExpectTable, ExpectTableEvent};
use crate::kv_table::{KvMode, KvTable, KvTableEvent, ReadonlyRow};
use requestbaby_core::config::Protocol;
use requestbaby_core::engine::grpc::MethodInfo;
use requestbaby_core::suggest::Addition;

#[path = "case_editor_proto.rs"]
mod proto;
#[path = "case_editor_ui.rs"]
mod ui;
use requestbaby_core::{fmt, inspect::{Segment, VarsPreview}, validate::ValidationIssue};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

const METHODS: [&str; 7] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorTab {
    Request,
    Expect,
    Scripts,
    Advanced,
    Vars,
    Json,
}

impl EditorTab {
    const ALL: [EditorTab; 6] = [EditorTab::Request, EditorTab::Expect, EditorTab::Scripts, EditorTab::Advanced, EditorTab::Vars, EditorTab::Json];

    fn label(self) -> &'static str {
        match self {
            EditorTab::Request => "请求",
            EditorTab::Expect => "断言",
            EditorTab::Scripts => "脚本（进阶）",
            EditorTab::Advanced => "高级",
            EditorTab::Vars => "变量",
            EditorTab::Json => "JSON",
        }
    }
}

pub enum CaseEditorEvent {
    /// 已保存到磁盘（rel）
    Saved,
    /// 请求发送当前用例（保存后发送）
    Send,
    /// 编辑某个变量：defined_in 为定义它的文件（相对项目根）；None = 未定义，需新增
    EditVar { name: String, defined_in: Option<String> },
    /// 打开变量面板
    OpenVars,
}

pub struct CaseEditor {
    /// 本次会话已捕获的运行时变量
    session_vars: BTreeMap<String, String>,
    rel: Option<String>,
    path: Option<PathBuf>,
    /// 最近一次加载/提交的文档：表单未覆盖的字段从这里保留
    base: Value,
    saved_text: String,
    dirty: bool,
    tab: EditorTab,
    env: String,
    issues: Vec<ValidationIssue>,
    /// 集合链继承的请求头名（浅 → 深，用例同名覆盖）
    inherited_headers: Vec<ReadonlyRow>,
    error: Option<String>,

    name: Entity<InputState>,
    /// 协议：决定请求页的界面（HTTP 方法 + URL / gRPC 地址 + 方法 + 消息 / WebSocket 地址 + 对话步骤）
    protocol: Protocol,
    protocol_select: Entity<SelectState<Vec<SharedString>>>,
    // ---- gRPC
    grpc_target: Entity<InputState>,
    grpc_method: Entity<InputState>,
    grpc_message: Entity<EditorState>,
    /// 多条消息（客户端流 / 双向流）
    grpc_streaming: bool,
    grpc_metadata: Entity<KvTable>,
    /// gRPC 子页：0 消息 / 1 元数据
    grpc_tab: usize,
    /// 服务的方法列表（反射 / .proto）；None = 还没加载
    grpc_methods: Option<Result<Vec<MethodInfo>, String>>,
    grpc_loading: bool,
    _grpc_task: Option<Task<()>>,
    // ---- WebSocket
    ws_url: Entity<InputState>,
    ws_headers: Entity<KvTable>,
    ws_query: Entity<KvTable>,
    ws_subprotocols: Entity<InputState>,
    ws_steps: Vec<proto::StepRow>,
    /// WebSocket 子页：0 对话 / 1 请求头 / 2 参数
    ws_tab: usize,
    _step_subs: Vec<Subscription>,
    // ---- Web / Android / Mac 桌面
    /// Web：要打开的页面；Android：应用包名；Mac 桌面：应用名称 / bundle ID
    ui_target: Entity<InputState>,
    /// Mac 桌面：窗口标题包含的文字
    ui_window: Entity<InputState>,
    /// Mac 桌面：开始前先退出应用
    ui_restart: bool,
    ui_activity: Entity<InputState>,
    ui_apk: Entity<InputState>,
    ui_clear_data: bool,
    ui_steps: Vec<ui::UiRow>,
    /// Android / Mac 桌面子页：0 步骤 / 1 应用
    ui_tab: usize,
    /// 读取到的页面 / 界面元素（步骤里点选定位）
    ui_snapshot: Option<Result<Vec<requestbaby_core::protocols::SnapElement>, String>>,
    ui_snapshot_loading: bool,
    _ui_task: Option<Task<()>>,
    _ui_subs: Vec<Subscription>,
    method: Entity<SelectState<Vec<SharedString>>>,
    url: Entity<InputState>,
    expect_status: Entity<InputState>,
    /// 文件里状态码用的是旧写法 expectStatus：原位写回（fmt --fix 迁移）
    legacy_status: bool,
    expect_body: Entity<ExpectTable>,
    expect_headers: Entity<ExpectTable>,
    capture: Entity<KvTable>,
    headers: Entity<KvTable>,
    query: Entity<KvTable>,
    /// 请求区子页：0 参数 / 1 请求头 / 2 请求体
    req_tab: usize,
    /// 请求体类型：0 无 / 1 raw / 2 表单 / 3 文件上传（multipart）
    body_mode: usize,
    /// raw 的格式：true = JSON（{"json"} 保存，自动 Content-Type），false = Text（{"text"} 原样）
    raw_json: bool,
    body: Entity<EditorState>,
    form: Entity<KvTable>,
    multipart: Entity<KvTable>,
    tests: Entity<EditorState>,
    pre: Entity<EditorState>,
    post: Entity<EditorState>,
    json: Entity<EditorState>,
    // ---- 高级
    tags: Entity<InputState>,
    timeout: Entity<InputState>,
    retry: Entity<InputState>,
    skip: bool,
    insecure: bool,
    follow_redirects: bool,
    /// 数据驱动：0 无 / 1 内联 / 2 文件
    data_mode: usize,
    cases_inline: Entity<EditorState>,
    cases_file: Entity<InputState>,
    /// 变量解析预览（core::inspect）
    vars_preview: Option<Result<VarsPreview, String>>,
    _validate_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<CaseEditorEvent> for CaseEditor {}

fn code_editor(language: &str, placeholder: &str, window: &mut Window, cx: &mut Context<EditorState>) -> EditorState {
    EditorState::new(window, cx)
        .language(language.to_string())
        .line_number(true)
        .soft_wrap(true)
        .placeholder(placeholder.to_string())
}

impl CaseEditor {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("用例名（缺省为文件名）"));
        let url = cx.new(|cx| InputState::new(window, cx).placeholder("{{baseUrl}}/path"));
        let expect_status = cx.new(|cx| InputState::new(window, cx).placeholder("200 / 200, 201 / {{status}}"));
        let methods: Vec<SharedString> = METHODS.iter().map(|m| SharedString::from(*m)).collect();
        let method = cx.new(|cx| SelectState::new(methods, Some(IndexPath::new(0)), window, cx));
        let headers = cx.new(|_| KvTable::new(KvMode::Toggle, "请求头名"));
        let query = cx.new(|_| KvTable::new(KvMode::Toggle, "参数名"));
        let form = cx.new(|_| KvTable::new(KvMode::Plain, "字段名"));
        let multipart = cx.new(|_| KvTable::new(KvMode::Multipart, "字段名"));
        let expect_body = cx.new(|_| ExpectTable::new("字段路径", "code / data.items[0].id / data.list[*].price"));
        let expect_headers = cx.new(|_| ExpectTable::new("响应头", "Content-Type"));
        let capture = cx.new(|_| KvTable::new(KvMode::Plain, "变量名").value_label("来源：data.token 或 header:X-Trace-Id"));
        let body = cx.new(|cx| code_editor("json", "JSON 请求体；非 JSON 文本按原样发送", window, cx));
        let tests = cx.new(|cx| code_editor("javascript", "每行一条 JS 断言（「断言」页写不出的复杂判断才用这里），如 data.list.every(x => x.price > 0)", window, cx));
        let pre = cx.new(|cx| code_editor("javascript", "请求前执行，可改写 req", window, cx));
        let post = cx.new(|cx| code_editor("javascript", "响应后执行（取变量优先用「断言」页的捕获），如 vars.ids = data.list.map(x => x.id).join(',')", window, cx));
        let json = cx.new(|cx| code_editor("json", "", window, cx));
        let tags = cx.new(|cx| InputState::new(window, cx).placeholder("smoke, auth（逗号分隔，与集合标签合并）"));
        let timeout = cx.new(|cx| InputState::new(window, cx).placeholder("缺省用项目 timeoutMs"));
        let retry = cx.new(|cx| InputState::new(window, cx).placeholder("0"));
        let cases_inline = cx.new(|cx| code_editor("json", "[{\"label\": \"正常\", \"sku\": \"A1\"}]", window, cx));
        let cases_file = cx.new(|cx| InputState::new(window, cx).placeholder("./data/users.csv（相对用例目录；CSV 首行表头或 JSON 数组）"));
        let protocols: Vec<SharedString> = Protocol::ALL.iter().map(|p| SharedString::from(p.label())).collect();
        let protocol_select = cx.new(|cx| SelectState::new(protocols, Some(IndexPath::new(0)), window, cx));
        let grpc_target = cx.new(|cx| InputState::new(window, cx).placeholder("{{grpcTarget}}（host:port；TLS 写 grpcs://host:port）"));
        let grpc_method = cx.new(|cx| InputState::new(window, cx).placeholder("包.服务/方法，如 user.v1.UserService/GetUser"));
        let grpc_message = cx.new(|cx| code_editor("json", "请求消息（JSON；字段名同 .proto）；选好方法后点「生成骨架」自动填写", window, cx));
        let grpc_metadata = cx.new(|_| KvTable::new(KvMode::Toggle, "元数据名（小写）"));
        let ws_url = cx.new(|cx| InputState::new(window, cx).placeholder("{{wsBase}}/path（ws:// 或 wss://）"));
        let ws_headers = cx.new(|_| KvTable::new(KvMode::Toggle, "请求头名"));
        let ws_query = cx.new(|_| KvTable::new(KvMode::Toggle, "参数名"));
        let ws_subprotocols = cx.new(|cx| InputState::new(window, cx).placeholder("chat.v1（服务端要求子协议时填写，逗号分隔）"));
        let ui_target = cx.new(|cx| InputState::new(window, cx).placeholder("{{webBase}}/login（要打开的页面）"));
        let ui_window = cx.new(|cx| InputState::new(window, cx).placeholder("如 Untitled（窗口标题里的一段文字）"));
        let ui_activity = cx.new(|cx| InputState::new(window, cx).placeholder(".MainActivity"));
        let ui_apk = cx.new(|cx| InputState::new(window, cx).placeholder("build/app-debug.apk"));

        let mut subs = vec![];
        for input in [&name, &url, &expect_status, &tags, &timeout, &retry, &cases_file, &grpc_target, &grpc_method, &ws_url, &ws_subprotocols, &ui_target, &ui_window, &ui_activity, &ui_apk] {
            subs.push(cx.subscribe_in(input, window, |this, _, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.refresh_dirty(cx);
                }
            }));
        }
        for table in [&expect_body, &expect_headers] {
            subs.push(cx.subscribe(table, |this, _, _: &ExpectTableEvent, cx| {
                this.refresh_dirty(cx);
                cx.notify();
            }));
        }
        for table in [&headers, &query, &form, &multipart, &capture, &grpc_metadata, &ws_headers, &ws_query] {
            subs.push(cx.subscribe(table, |this, _, _: &KvTableEvent, cx| {
                this.refresh_dirty(cx);
                cx.notify();
            }));
        }
        for editor in [&body, &tests, &pre, &post, &json, &cases_inline, &grpc_message] {
            subs.push(cx.subscribe_in(editor, window, |this, _, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.refresh_dirty(cx);
                }
            }));
        }
        subs.push(cx.subscribe_in(&method, window, |this, _, _: &SelectEvent<Vec<SharedString>>, _, cx| {
            this.refresh_dirty(cx);
        }));
        subs.push(cx.subscribe_in(&protocol_select, window, |this, _, ev: &SelectEvent<Vec<SharedString>>, window, cx| {
            if let SelectEvent::Confirm(Some(label)) = ev {
                if let Some(p) = Protocol::ALL.iter().find(|p| p.label() == label.as_ref()) {
                    this.set_protocol(*p, window, cx);
                }
            }
        }));

        Self {
            session_vars: BTreeMap::new(),
            rel: None,
            path: None,
            base: Value::Null,
            saved_text: String::new(),
            dirty: false,
            tab: EditorTab::Request,
            env: String::new(),
            issues: vec![],
            inherited_headers: vec![],
            error: None,
            name,
            protocol: Protocol::Http,
            protocol_select,
            grpc_target,
            grpc_method,
            grpc_message,
            grpc_streaming: false,
            grpc_metadata,
            grpc_tab: 0,
            grpc_methods: None,
            grpc_loading: false,
            _grpc_task: None,
            ws_url,
            ws_headers,
            ws_query,
            ws_subprotocols,
            ws_steps: vec![],
            ws_tab: 0,
            _step_subs: vec![],
            ui_target,
            ui_window,
            ui_restart: false,
            ui_activity,
            ui_apk,
            ui_clear_data: false,
            ui_steps: vec![],
            ui_tab: 0,
            ui_snapshot: None,
            ui_snapshot_loading: false,
            _ui_task: None,
            _ui_subs: vec![],
            method,
            url,
            expect_status,
            legacy_status: false,
            expect_body,
            expect_headers,
            capture,
            headers,
            query,
            req_tab: 0,
            body_mode: 0,
            raw_json: true,
            body,
            form,
            multipart,
            tests,
            pre,
            post,
            json,
            tags,
            timeout,
            retry,
            skip: false,
            insecure: false,
            follow_redirects: false,
            data_mode: 0,
            cases_inline,
            cases_file,
            vars_preview: None,
            _validate_task: None,
            _subscriptions: subs,
        }
    }

    pub fn rel(&self) -> Option<&str> {
        self.rel.as_deref()
    }

    pub fn path(&self) -> Option<&PathBuf> {
        self.path.as_ref()
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn set_env(&mut self, env: String, cx: &mut Context<Self>) {
        self.env = env;
        self.validate_now(cx);
    }

    /// 本次会话已捕获的运行时变量（预览中直接显示其值；「发送」会带上它们）
    pub fn set_session_vars(&mut self, vars: BTreeMap<String, String>, cx: &mut Context<Self>) {
        if self.session_vars != vars {
            self.session_vars = vars;
            cx.notify();
        }
    }

    /// 追加断言 / 捕获到编辑中的内容（同一路径 / 变量名覆盖），切到「断言」页展示
    pub fn append(&mut self, add: &Addition, window: &mut Window, cx: &mut Context<Self>) {
        match add {
            Addition::Expect { path, matcher } => self.expect_body.update(cx, |t, cx| t.upsert(path, matcher.clone(), window, cx)),
            Addition::Capture { name, from } => self.capture.update(cx, |t, cx| t.upsert(name, Value::String(from.clone()), window, cx)),
        }
        self.set_tab(EditorTab::Expect, window, cx);
        self.refresh_dirty(cx);
        cx.notify();
    }

    /// 打开用例文件（未保存的修改由调用方先确认）
    pub fn open(&mut self, rel: String, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        // 集合链继承的请求头（浅 → 深，深层同名覆盖浅层）：在请求头表格下方只读展示
        self.inherited_headers = requestbaby_core::runner::discover::discover(std::slice::from_ref(&path))
            .ok()
            .and_then(|v| v.into_iter().next())
            .map(|dc| {
                let mut rows: Vec<ReadonlyRow> = vec![];
                for c in &dc.chain {
                    let source = if c.name.is_empty() { "继承自集合".to_string() } else { format!("继承自集合 {}", c.name) };
                    for (k, v) in requestbaby_core::config::enabled_kv(&c.headers) {
                        rows.retain(|r| !r.key.eq_ignore_ascii_case(k));
                        rows.push(ReadonlyRow { key: k.clone(), value: v.to_string(), source: source.clone() });
                    }
                }
                rows
            })
            .unwrap_or_default();
        let dir = path.parent().map(|p| p.to_path_buf());
        self.multipart.update(cx, |t, _| t.set_base_dir(dir));
        self.rel = Some(rel);
        self.path = Some(path);
        self.issues.clear();
        match serde_json::from_str::<Value>(&text) {
            Ok(doc) => {
                self.error = None;
                self.base = doc;
                self.saved_text = fmt::canonical_case_text(&self.base);
                self.load_form(window, cx);
            }
            Err(e) => {
                // 解析失败：直接进入 JSON 视图修复
                self.error = Some(format!("JSON 解析失败: {e}"));
                self.base = Value::Null;
                self.saved_text = text.clone();
                self.tab = EditorTab::Json;
            }
        }
        self.json.update(cx, |s, cx| s.set_value(text, window, cx));
        self.validate_now(cx);
        self.dirty = false;
        cx.notify();
    }

    /// 关闭当前用例（文件被删除 / 移走）
    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.rel = None;
        self.path = None;
        self.dirty = false;
        self.issues.clear();
        self.error = None;
        cx.notify();
    }

    /// 外部（AI / CLI / 编辑器）修改了当前文件：未编辑时静默重载
    pub fn reload_if_clean(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let (Some(rel), Some(path)) = (self.rel.clone(), self.path.clone()) else {
            return false;
        };
        if self.dirty {
            return false;
        }
        let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
        let canonical = serde_json::from_str::<Value>(&on_disk).map(|v| fmt::canonical_case_text(&v)).unwrap_or(on_disk);
        if canonical == self.saved_text {
            return false;
        }
        let tab = self.tab;
        self.open(rel, path, window, cx);
        if self.error.is_none() {
            self.tab = tab;
        }
        true
    }

    fn load_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let f = CaseForm::from_doc(&self.base);
        self.legacy_status = f.legacy_status;
        self.expect_body.update(cx, |t, cx| t.load(Some(&f.expect_body), window, cx));
        self.expect_headers.update(cx, |t, cx| t.load(Some(&f.expect_headers), window, cx));
        self.capture.update(cx, |t, cx| t.load(Some(&f.capture), window, cx));
        self.raw_json = f.body.raw_json;
        self.body_mode = f.body.mode;
        // 打开用例时落在最有内容的子页：有请求体 → 请求体；有参数 → 参数；否则请求头
        self.req_tab = if f.body.mode != 0 { 2 } else if !f.query.is_empty() { 0 } else { 1 };
        self.headers.update(cx, |t, cx| t.load(Some(&f.headers), window, cx));
        self.query.update(cx, |t, cx| t.load(Some(&f.query), window, cx));
        self.form.update(cx, |t, cx| t.load(Some(&f.body.form), window, cx));
        self.multipart.update(cx, |t, cx| t.load(Some(&f.body.multipart), window, cx));
        self.sync_header_hints(cx);
        self.skip = f.skip;
        self.insecure = f.insecure;
        self.follow_redirects = f.follow_redirects;
        self.data_mode = f.data_mode;
        for (input, value) in [
            (&self.name, &f.name),
            (&self.url, &f.url),
            (&self.expect_status, &f.status),
            (&self.tags, &f.tags),
            (&self.timeout, &f.timeout),
            (&self.retry, &f.retry),
            (&self.cases_file, &f.cases_file),
        ] {
            input.update(cx, |st, cx| st.set_value(value.clone(), window, cx));
        }
        for (editor, value) in [(&self.body, &f.body.text), (&self.tests, &f.tests), (&self.pre, &f.pre), (&self.post, &f.post), (&self.cases_inline, &f.cases_inline)] {
            editor.update(cx, |st, cx| st.set_value(value.clone(), window, cx));
        }
        let method: SharedString = if f.method.is_empty() { "GET".into() } else { f.method.clone().into() };
        self.method.update(cx, |st, cx| st.set_selected_value(&method, window, cx));
        // 协议相关
        let protocol_changed = self.protocol != f.protocol;
        self.protocol = f.protocol;
        let label: SharedString = f.protocol.label().into();
        self.protocol_select.update(cx, |st, cx| st.set_selected_value(&label, window, cx));
        for (input, value) in [
            (&self.grpc_target, &f.grpc.target),
            (&self.grpc_method, &f.grpc.method),
            (&self.ws_url, &f.ws.url),
            (&self.ws_subprotocols, &f.ws.subprotocols),
            (&self.ui_target, &f.ui.target),
            (&self.ui_window, &f.ui.window),
            (&self.ui_activity, &f.ui.activity),
            (&self.ui_apk, &f.ui.apk),
        ] {
            input.update(cx, |st, cx| st.set_value(value.clone(), window, cx));
        }
        self.grpc_message.update(cx, |st, cx| st.set_value(f.grpc.message.clone(), window, cx));
        self.grpc_streaming = f.grpc.streaming;
        self.grpc_metadata.update(cx, |t, cx| t.load(Some(&f.grpc.metadata), window, cx));
        self.ws_headers.update(cx, |t, cx| t.load(Some(&f.ws.headers), window, cx));
        self.ws_query.update(cx, |t, cx| t.load(Some(&f.ws.query), window, cx));
        self.set_steps(&f.ws.steps, window, cx);
        self.ui_clear_data = f.ui.clear_data;
        self.ui_restart = f.ui.restart;
        self.set_ui_steps(&f.ui.steps, window, cx);
        self.ui_tab = 0;
        if protocol_changed {
            self.ui_snapshot = None;
        }
        self.sync_ui_target_placeholder(window, cx);
        self.ws_tab = 0;
        self.grpc_tab = 0;
        if protocol_changed || self.grpc_methods.as_ref().is_some_and(|m| m.is_err()) {
            self.grpc_methods = None;
        }
        if self.protocol == Protocol::Grpc && self.grpc_methods.is_none() {
            self.load_grpc_methods(cx);
        }
        self.sync_proto_hints(cx);
    }

    /// 当前编辑内容 → 文档（JSON 视图以文本为准；其余视图只改动过的字段，见 case_form）
    fn collect(&self, cx: &App) -> Result<Value, String> {
        if self.tab == EditorTab::Json {
            return serde_json::from_str::<Value>(&self.json.read(cx).value()).map_err(|e| format!("JSON 解析失败: {e}"));
        }
        let input = |e: &Entity<InputState>| e.read(cx).value().to_string();
        let editor = |e: &Entity<EditorState>| e.read(cx).value().to_string();
        let steps: Vec<StepForm> = self
            .ws_steps
            .iter()
            .map(|r| StepForm { action: r.action, content: input(&r.content), capture: input(&r.capture), timeout: input(&r.timeout), raw: r.raw.clone() })
            .collect();
        let form = CaseForm {
            name: input(&self.name),
            protocol: self.protocol,
            grpc: GrpcForm {
                target: input(&self.grpc_target),
                method: input(&self.grpc_method),
                message: editor(&self.grpc_message),
                streaming: self.grpc_streaming,
                metadata: self.grpc_metadata.read(cx).to_map(cx),
            },
            ws: WsForm {
                url: input(&self.ws_url),
                headers: self.ws_headers.read(cx).to_map(cx),
                query: self.ws_query.read(cx).to_map(cx),
                subprotocols: input(&self.ws_subprotocols),
                steps,
            },
            ui: UiForm {
                target: input(&self.ui_target),
                window: input(&self.ui_window),
                restart: self.ui_restart,
                activity: input(&self.ui_activity),
                apk: input(&self.ui_apk),
                clear_data: self.ui_clear_data,
                steps: self.current_ui_steps(cx),
            },
            method: self.method.read(cx).selected_value().map(|m| m.to_string()).unwrap_or_else(|| "GET".into()),
            url: input(&self.url),
            status: input(&self.expect_status),
            legacy_status: self.legacy_status,
            headers: self.headers.read(cx).to_map(cx),
            query: self.query.read(cx).to_map(cx),
            body: BodyForm {
                mode: self.body_mode,
                raw_json: self.raw_json,
                text: editor(&self.body),
                form: self.form.read(cx).to_map(cx),
                multipart: self.multipart.read(cx).to_map(cx),
            },
            expect_headers: self.expect_headers.read(cx).to_map(cx)?,
            expect_body: self.expect_body.read(cx).to_map(cx)?,
            capture: self.capture.read(cx).to_map(cx),
            tests: editor(&self.tests),
            pre: editor(&self.pre),
            post: editor(&self.post),
            tags: input(&self.tags),
            timeout: input(&self.timeout),
            retry: input(&self.retry),
            skip: self.skip,
            insecure: self.insecure,
            follow_redirects: self.follow_redirects,
            data_mode: self.data_mode,
            cases_inline: editor(&self.cases_inline),
            cases_file: input(&self.cases_file),
        };
        form.to_doc(&self.base)
    }

    fn refresh_dirty(&mut self, cx: &mut Context<Self>) {
        if self.rel.is_none() {
            return;
        }
        let dirty = match self.collect(cx) {
            Ok(d) => fmt::canonical_case_text(&d) != self.saved_text,
            Err(_) => true,
        };
        if dirty != self.dirty {
            self.dirty = dirty;
            cx.notify();
        }
    }

    pub fn set_tab(&mut self, tab: EditorTab, window: &mut Window, cx: &mut Context<Self>) {
        if tab == self.tab {
            return;
        }
        if tab == EditorTab::Json {
            // 表单 → JSON：以规范形态展示
            if let Ok(d) = self.collect(cx) {
                self.base = d;
                let text = fmt::canonical_case_text(&self.base);
                self.json.update(cx, |s, cx| s.set_value(text, window, cx));
            }
        } else if self.tab == EditorTab::Json {
            // JSON → 表单：解析失败则留在 JSON 视图
            match self.collect(cx) {
                Ok(d) => {
                    self.error = None;
                    self.base = d;
                    self.load_form(window, cx);
                }
                Err(e) => {
                    self.error = Some(e);
                    cx.notify();
                    return;
                }
            }
        }
        self.tab = tab;
        cx.notify();
    }

    pub fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let (Some(_), Some(path)) = (self.rel.clone(), self.path.clone()) else {
            return false;
        };
        let doc = match self.collect(cx) {
            Ok(d) => d,
            Err(e) => {
                self.error = Some(e);
                cx.notify();
                return false;
            }
        };
        let text = fmt::canonical_case_text(&doc);
        if let Err(e) = requestbaby_core::fsx::write(&path, &text) {
            self.error = Some(format!("写入失败: {e}"));
            cx.notify();
            return false;
        }
        self.error = None;
        self.base = doc;
        self.saved_text = text.clone();
        if self.tab == EditorTab::Json {
            self.json.update(cx, |s, cx| s.set_value(text, window, cx));
        }
        self.dirty = false;
        self.validate_now(cx);
        cx.emit(CaseEditorEvent::Saved);
        cx.notify();
        true
    }

    /// 校验（全项目上下文，跨用例捕获的变量不误报）+ 变量解析预览
    /// 变量文件在外部变化后重新计算校验与变量预览
    pub fn refresh_vars(&mut self, cx: &mut Context<Self>) {
        self.validate_now(cx);
        cx.notify();
    }

    /// 后台计算（全项目上下文，项目大时不卡界面）；新的一次替换旧任务，切换文件后旧结果作废
    fn validate_now(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.path.clone() else { return };
        let env = if self.env.is_empty() { "dev".to_string() } else { self.env.clone() };
        let target = path.clone();
        let task = cx.background_spawn(async move {
            let issues = requestbaby_core::validate::validate_file(&path, &env).unwrap_or_default();
            let preview = requestbaby_core::inspect::case_vars(&path, &env).map_err(|e| format!("{e:#}"));
            (issues, preview)
        });
        self._validate_task = Some(cx.spawn(async move |this, cx| {
            let (issues, preview) = task.await;
            let _ = this.update(cx, |e, cx| {
                if e.path.as_ref() == Some(&target) {
                    e.issues = issues;
                    e.vars_preview = Some(preview);
                    cx.notify();
                }
            });
        }));
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rel = self.rel.clone().unwrap_or_default();
        let errors = self.issues.iter().filter(|i| i.level == "error").count();
        let warns = self.issues.iter().filter(|i| i.level != "error" && !i.is_style()).count();
        h_flex()
            .px_4()
            .py_2()
            .gap_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap_2()
                            .min_w_0()
                            .child(
                                div()
                                    .id("case-title")
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(rel.clone())
                                    .tooltip({
                                        let rel = rel.clone();
                                        move |window, cx| gpui_kit::component::tooltip::Tooltip::new(rel.clone()).build(window, cx)
                                    }),
                            )
                            .child(crate::style::cli_button("case-cli", format!("requestbaby send {rel} -e {}", self.env)).flex_shrink_0())
                            .when(self.dirty, |this| {
                                this.child(Tag::warning().outline().xsmall().flex_shrink_0().child("未保存"))
                            }),
                    ),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .gap_2()
                    .when(errors > 0, |this| this.child(Tag::danger().outline().xsmall().child(format!("{errors} 个错误"))))
                    .when(warns > 0, |this| this.child(Tag::warning().outline().xsmall().child(format!("{warns} 个警告"))))
                    .when(errors == 0 && warns == 0 && self.rel.is_some(), |this| {
                        this.child(Tag::success().outline().xsmall().child("校验通过"))
                    })
                    .child(
                        Button::new("save-case")
                            .small()
                            .outline()
                            .icon(IconName::Check)
                            .label("保存")
                            .disabled(!self.dirty)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save(window, cx);
                            })),
                    )
                    .child(
                        Button::new("send-case")
                            .small()
                            .primary()
                            .rounded(style::PILL)
                            .icon(IconName::Play)
                            .label("发送")
                            .on_click(cx.listener(|this, _, window, cx| {
                                if this.dirty && !this.save(window, cx) {
                                    return;
                                }
                                cx.emit(CaseEditorEvent::Send);
                            })),
                    ),
            )
    }

    fn render_request(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let preview = self.render_var_preview(cx);
        let muted = cx.theme().muted_foreground;
        let label = |text: &'static str| div().text_xs().font_weight(FontWeight::MEDIUM).text_color(muted).child(text);
        // 整页滚动：参数 / 请求头表格变长、变量提示换行时不挤压，也不盖住下方的校验栏
        v_flex()
            .id("request-page")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_4()
            .gap_3()
            .child(self.render_address_bar(cx))
            .child(preview)
            .child(v_flex().gap_1().child(label("名称")).child(Input::new(&self.name).small()))
            .map(|this| match self.protocol {
                Protocol::Http => this.child(self.render_request_parts(cx).into_any_element()),
                Protocol::Grpc => this.child(self.render_grpc_parts(cx)),
                Protocol::Ws => this.child(self.render_ws_parts(cx)),
                Protocol::Web | Protocol::Android | Protocol::Desktop => this.child(self.render_ui_parts(cx)),
            })
    }

    /// 顶部地址栏：协议 + （HTTP：方法 + URL / gRPC：地址 + 方法 / WebSocket：地址）
    fn render_address_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let bar = h_flex().gap_2().child(div().w(px(140.)).child(Select::new(&self.protocol_select).small()));
        match self.protocol {
            Protocol::Http => bar
                .child(div().w(px(110.)).child(Select::new(&self.method).small()))
                .child(div().flex_1().min_w_0().child(Input::new(&self.url).small()))
                .into_any_element(),
            Protocol::Grpc => bar
                // 方法名通常比地址长：地址固定窄一些，方法占剩余宽度
                .child(div().w(px(180.)).flex_shrink_0().child(Input::new(&self.grpc_target).small()))
                .child(div().flex_1().min_w_0().child(Input::new(&self.grpc_method).small()))
                .child(self.render_method_picker(cx))
                .into_any_element(),
            Protocol::Ws => bar.child(div().flex_1().min_w_0().child(Input::new(&self.ws_url).small())).into_any_element(),
            Protocol::Web | Protocol::Android | Protocol::Desktop => bar
                .child(div().flex_1().min_w_0().child(Input::new(&self.ui_target).small()))
                .child(self.render_snapshot_button(cx))
                .into_any_element(),
        }
    }

    /// 当前协议的地址（变量预览的第一行）
    fn address(&self, cx: &App) -> String {
        match self.protocol {
            Protocol::Http => self.url.read(cx).value().to_string(),
            Protocol::Grpc => self.grpc_target.read(cx).value().to_string(),
            Protocol::Ws => self.ws_url.read(cx).value().to_string(),
            Protocol::Web | Protocol::Android | Protocol::Desktop => self.ui_target.read(cx).value().to_string(),
        }
    }

    /// 请求头 / 参数 / 请求体中的全部文本（变量引用预览用）
    fn request_texts(&self, cx: &App) -> String {
        match self.protocol {
            Protocol::Grpc => {
                return [self.grpc_method.read(cx).value().to_string(), self.grpc_metadata.read(cx).texts(cx), self.grpc_message.read(cx).value().to_string()].join("\n");
            }
            Protocol::Ws => {
                let mut t = vec![self.ws_headers.read(cx).texts(cx), self.ws_query.read(cx).texts(cx)];
                t.extend(self.ws_steps.iter().map(|r| r.content.read(cx).value().to_string()));
                return t.join("\n");
            }
            Protocol::Web | Protocol::Android | Protocol::Desktop => {
                let t: Vec<String> = self.current_ui_steps(cx).into_iter().flat_map(|s| [s.target, s.value, s.capture]).collect();
                return t.join("\n");
            }
            Protocol::Http => {}
        }
        let mut t = vec![self.headers.read(cx).texts(cx), self.query.read(cx).texts(cx)];
        match self.body_mode {
            1 => t.push(self.body.read(cx).value().to_string()),
            2 => t.push(self.form.read(cx).texts(cx)),
            3 => t.push(self.multipart.read(cx).texts(cx)),
            _ => {}
        }
        t.join("\n")
    }

    /// 请求头表格下方的只读行：继承自集合的 + 按请求体自动生成的 Content-Type（与实际发送一致）
    fn sync_header_hints(&mut self, cx: &mut Context<Self>) {
        let mut rows = self.inherited_headers.clone();
        let auto = match self.body_mode {
            1 if self.raw_json => Some("application/json"),
            2 => Some("application/x-www-form-urlencoded"),
            3 => Some("multipart/form-data; boundary=<自动生成>"),
            _ => None,
        };
        if let Some(ct) = auto {
            rows.retain(|r| !r.key.eq_ignore_ascii_case("content-type"));
            rows.push(ReadonlyRow { key: "Content-Type".into(), value: ct.into(), source: "按请求体自动".into() });
        }
        self.headers.update(cx, |t, cx| t.set_readonly(rows, cx));
    }

    fn set_body_mode(&mut self, mode: usize, cx: &mut Context<Self>) {
        self.body_mode = mode;
        self.sync_header_hints(cx);
        self.refresh_dirty(cx);
        cx.notify();
    }

    fn set_raw_json(&mut self, json: bool, cx: &mut Context<Self>) {
        self.raw_json = json;
        self.sync_header_hints(cx);
        self.refresh_dirty(cx);
        cx.notify();
    }

    /// raw JSON 格式化（Postman 的 Beautify）
    fn beautify_body(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.body.read(cx).value().to_string();
        match serde_json::from_str::<Value>(&text) {
            Ok(v) => {
                let pretty = serde_json::to_string_pretty(&v).unwrap_or(text);
                self.body.update(cx, |s, cx| s.set_value(pretty, window, cx));
            }
            Err(e) => {
                self.error = Some(format!("不是合法 JSON，无法格式化：{e}"));
                cx.notify();
            }
        }
    }

    /// 参数 / 请求头 / 请求体 子页（参照 Postman：标签带计数，请求体按类型切换）
    fn render_request_parts(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, success, border, radius) = (theme.muted_foreground, theme.success, theme.border, theme.radius);
        let badge = |n: usize| div().text_xs().text_color(success).child(format!("{n}"));
        let dot = || div().size(px(6.)).rounded_full().bg(success);
        let query_n = self.query.read(cx).active_count(cx);
        let header_n = self.headers.read(cx).active_count(cx);
        let has_body = self.body_mode != 0;
        let hint = |t: &'static str| div().text_xs().text_color(muted).child(t);
        // 显示顺序与 Postman 一致：none / form-data / x-www-form-urlencoded / raw；值为内部 body_mode
        const BODY_ORDER: [usize; 4] = [0, 3, 2, 1];
        let body_ix = BODY_ORDER.iter().position(|m| *m == self.body_mode).unwrap_or(0);
        let raw_text = self.body.read(cx).value().to_string();
        let invalid_json =
            self.body_mode == 1 && self.raw_json && !raw_text.trim().is_empty() && serde_json::from_str::<Value>(&raw_text).is_err();

        let tabs = TabBar::new("req-parts")
            .underline()
            .small()
            .selected_index(self.req_tab)
            .child(Tab::new().label("参数").when(query_n > 0, |t| t.suffix(badge(query_n))))
            .child(Tab::new().label("请求头").when(header_n > 0, |t| t.suffix(badge(header_n))))
            .child(Tab::new().label("请求体").when(has_body, |t| t.suffix(dot())))
            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                this.req_tab = *ix;
                cx.notify();
            }));

        v_flex()
            .gap_2()
            .child(tabs)
            .map(|this| match self.req_tab {
                0 => this
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().text_xs().font_weight(FontWeight::MEDIUM).child("查询参数"))
                            .child(hint("URL 编码后追加到 URL（URL 里已写的 ?a=1 保留）；取消勾选 = 保留但不发送")),
                    )
                    .child(self.query.clone()),
                1 => this
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().text_xs().font_weight(FontWeight::MEDIUM).child("请求头"))
                            .child(hint("值支持 {{var}}；表格下方灰色行是继承自集合 / 自动生成的，会一起发送")),
                    )
                    .child(self.headers.clone()),
                _ => this
                    .child(
                        h_flex()
                            .gap_3()
                            .child(div().flex_shrink_0().child(
                                RadioGroup::horizontal("body-mode")
                                    .children(["无", "form-data", "x-www-form-urlencoded", "raw"])
                                    .selected_index(Some(body_ix))
                                    .on_change(cx.listener(|this, ix: &usize, _, cx| this.set_body_mode(BODY_ORDER[*ix], cx))),
                            ))
                            .when(self.body_mode == 1, |this| {
                                this.child(
                                    ButtonGroup::new("raw-kind")
                                        .xsmall()
                                        .outline()
                                        .compact()
                                        .child(Button::new("raw-json").label("JSON").selected(self.raw_json))
                                        .child(Button::new("raw-text").label("Text").selected(!self.raw_json))
                                        .on_click(cx.listener(|this, clicks: &Vec<usize>, _, cx| {
                                            if let Some(ix) = clicks.first() {
                                                this.set_raw_json(*ix == 0, cx);
                                            }
                                        })),
                                )
                                .child(div().flex_1())
                                .when(self.raw_json, |this| {
                                    this.child(
                                        Button::new("beautify")
                                            .xsmall()
                                            .ghost()
                                            .label("格式化")
                                            .on_click(cx.listener(|this, _, window, cx| this.beautify_body(window, cx))),
                                    )
                                })
                            }),
                    )
                    .map(|this| match self.body_mode {
                        1 => this
                            .child(
                                div()
                                    .h(px(280.))
                                    .rounded(radius)
                                    .border_1()
                                    .border_color(border)
                                    .overflow_hidden()
                                    .child(Editor::new(&self.body).h_full()),
                            )
                            .child(if invalid_json {
                                div().text_xs().text_color(cx.theme().warning).child("不是合法 JSON：将按原文发送，也不会自动加 Content-Type（数字变量请写成 \"{{id:number}}\"）")
                            } else if self.raw_json {
                                hint("自动加 Content-Type: application/json；数字字段写 \"{{id:number}}\" 渲染为裸数字")
                            } else {
                                hint("按原文发送；需要的 Content-Type 请在请求头里写")
                            }),
                        2 => this.child(self.form.clone()).child(hint("逐字段 URL 编码；自动加 Content-Type: application/x-www-form-urlencoded")),
                        3 => this
                            .child(self.multipart.clone())
                            .child(hint("名称格右侧切换「文本 / 文件」；文件路径相对用例目录；Content-Type（含 boundary）自动生成，不要手写")),
                        _ => this.child(
                            div()
                                .h(px(120.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(radius)
                                .border_1()
                                .border_dashed()
                                .border_color(border)
                                .text_sm()
                                .text_color(muted)
                                .child("此请求没有请求体"),
                        ),
                    }),
            })
    }

    /// URL 解析预览 + 请求中引用的变量（值、来源；点击跳到定义处）
    fn render_var_preview(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(Ok(p)) = &self.vars_preview else { return div().into_any_element() };
        let muted = cx.theme().muted_foreground;
        let mono = cx.theme().mono_font_family.clone();
        let url = self.address(cx);
        let url_segments = p.segments(&url);
        if !url_segments.iter().any(|s| !matches!(s, Segment::Text { .. }))
            && !self.request_texts(cx).contains("{{")
        {
            return div().into_any_element();
        }
        let color = |s: &Segment| match s {
            Segment::Text { .. } => muted,
            Segment::Var { .. } => cx.theme().primary,
            Segment::Runtime { .. } => cx.theme().warning,
            Segment::Process { set: true, .. } => cx.theme().primary,
            Segment::Process { .. } | Segment::Undefined { .. } => cx.theme().danger,
        };
        let line = h_flex()
            .flex_wrap()
            .text_xs()
            .font_family(mono.clone())
            .child(div().pr_2().text_color(muted).child("→"))
            .children(url_segments.iter().map(|s| {
                let text = match s {
                    Segment::Text { text } => text.clone(),
                    Segment::Var { value, .. } => value.clone(),
                    Segment::Runtime { name, .. } => self.session_vars.get(name).cloned().unwrap_or_else(|| format!("{{{{{name}}}}}")),
                    Segment::Undefined { name } => format!("{{{{{name}}}}}"),
                    Segment::Process { name, .. } => format!("${name}"),
                };
                div().text_color(color(s)).child(text)
            }));

        // 请求中引用的变量（URL / 请求头 / 请求体，按出现顺序去重）
        let texts = [url, self.request_texts(cx)];
        let mut seen: Vec<String> = vec![];
        let mut refs: Vec<Segment> = vec![];
        for t in &texts {
            for s in p.segments(t) {
                let name = match &s {
                    Segment::Text { .. } => continue,
                    Segment::Var { name, .. } | Segment::Runtime { name, .. } | Segment::Undefined { name } => name.clone(),
                    Segment::Process { name, .. } => format!("%{name}"),
                };
                if !seen.contains(&name) {
                    seen.push(name);
                    refs.push(s);
                }
            }
        }
        let chips = refs.into_iter().enumerate().map(|(ix, s)| {
            let (name, detail, defined_in) = match &s {
                Segment::Var { name, value, source, defined_in } => (name.clone(), format!("= {value} · {source}"), Some(Some(defined_in.clone()))),
                Segment::Runtime { name, writers } => match self.session_vars.get(name) {
                    Some(v) => (name.clone(), format!("= {v} · 本次会话捕获，发送时带上"), None),
                    None => (name.clone(), format!("运行时捕获 ← {}（发送本用例时会自动先运行它）", writers.join(", ")), None),
                },
                Segment::Undefined { name } => (name.clone(), "未定义 · 点击添加到当前环境".to_string(), Some(None)),
                Segment::Process { name, set } => (format!("%{name}"), if *set { "进程环境变量".into() } else { "进程环境变量未设置".into() }, None),
                Segment::Text { .. } => unreachable!("已过滤"),
            };
            let c = color(&s);
            h_flex()
                .id(("var-ref", ix))
                .gap_1()
                .px_1p5()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(c.opacity(0.4))
                .text_xs()
                .child(div().font_family(mono.clone()).text_color(c).child(name.clone()))
                .child(div().text_color(muted).child(detail))
                .when_some(defined_in, |this, defined_in| {
                    this.cursor_pointer()
                        .hover(|d| d.bg(cx.theme().table_hover))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(CaseEditorEvent::EditVar { name: name.clone(), defined_in: defined_in.clone() })
                        }))
                })
        });
        v_flex()
            .gap_1()
            .child(line)
            .child(h_flex().flex_wrap().gap_1().children(chips))
            .into_any_element()
    }

    /// 「断言」页：expect（状态码 / 响应体 / 响应头）+ capture，与文件字段一一对应
    fn render_expect(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let mono = cx.theme().mono_font_family.clone();
        let section = |title: &'static str, field: &'static str, hint: &'static str| {
            h_flex()
                .gap_2()
                .items_baseline()
                .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(title))
                .child(div().text_xs().font_family(mono.clone()).text_color(muted).child(field))
                .child(div().text_xs().text_color(muted).child(hint))
        };
        let status_field = if self.legacy_status { "expectStatus（旧写法）" } else { "expect.status" };
        let (status_title, status_hint, body_title, body_hint, head_title, head_hint) = match self.protocol {
            Protocol::Http => ("状态码", "多个可接受的写成 200, 201；数据驱动按行不同写 {{status}}；留空则不检查", "响应体", "路径从响应体根写起；[*] 表示每个元素都要满足；值可以写 {{变量}}", "响应头", "名称不区分大小写"),
            Protocol::Grpc => ("gRPC 状态", "OK / NOT_FOUND / UNAUTHENTICATED…，多个写 OK, NOT_FOUND；留空则不检查", "响应消息", "字段名同 .proto；服务端流 / 双向流时是消息数组：[0].id、[*].state、$ 长度", "响应元数据", "headers + trailers；错误说明在 grpc-message"),
            Protocol::Ws => ("握手状态", "成功是 101；验证被拒写 401 / 403；留空 = 必须握手成功", "收到的消息", "全部收到的消息组成的数组：[0].type、[*].type、$ 长度；逐条断言写在「对话」里的接收步骤", "握手响应头", "服务端关闭时另有 ws-close-code / ws-close-reason"),
            Protocol::Web => ("页面状态", "打开的页面的 HTTP 状态，通常 200；留空则不检查", "结束时的页面", "{url, title}：如 url 写 { \"contains\": \"/orders\" }；界面上的断言写在「请求」页的步骤里", "页面响应头", "最后一次整页加载的响应头"),
            Protocol::Android => ("状态", "Android 没有状态码，留空", "结束时的界面", "{package, activity}：如 activity 写 { \"contains\": \"MainActivity\" }；界面上的断言写在「请求」页的步骤里", "响应头", "Android 没有响应头"),
            Protocol::Desktop => ("状态", "Mac 桌面应用没有状态码，留空", "结束时的窗口", "{app, window}：如 window 写 { \"contains\": \"Untitled\" }；界面上的断言写在「请求」页的步骤里", "响应头", "Mac 桌面应用没有响应头"),
        };
        v_flex()
            .id("case-expect")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_4()
            .gap_4()
            .child(
                v_flex()
                    .gap_1()
                    .child(section(status_title, status_field, status_hint))
                    .child(div().w(px(220.)).child(Input::new(&self.expect_status).small()))
                    .when(self.legacy_status, |this| {
                        this.child(div().text_xs().text_color(cx.theme().warning).child("文件里是旧写法 expectStatus：项目菜单「格式化项目」会改成 expect.status"))
                    }),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(section(body_title, "expect.body", body_hint))
                    .child(self.expect_body.clone()),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(section(head_title, "expect.headers", head_hint))
                    .child(self.expect_headers.clone()),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(section("捕获为变量", "capture", "后续用例用 {{变量名}} 引用；来源写响应体路径或 header:响应头名"))
                    .child(self.capture.clone()),
            )
            .child(div().text_xs().text_color(muted).child(
                "失败时报告写明「字段 · 期望 · 实际」。表格写不出的判断（跨字段计算、遍历比较）到「脚本（进阶）」页写 JS",
            ))
    }

    fn render_scripts(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let label = |text: &'static str, hint: &'static str| {
            h_flex()
                .gap_2()
                .child(div().text_xs().font_weight(FontWeight::MEDIUM).text_color(cx.theme().muted_foreground).child(text))
                .child(div().text_xs().text_color(cx.theme().muted_foreground).child(hint))
        };
        v_flex()
            .flex_1()
            .min_h_0()
            .p_4()
            .gap_3()
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_1()
                    .child(label("JS 断言 tests", "每行一条 JS 表达式，可用 res / data / vars / env；能在「断言」页表达的优先写在那里"))
                    .child(div().flex_1().min_h_0().child(Editor::new(&self.tests).h_full())),
            )
            .child(v_flex().gap_1().child(label("前置 pre", "")).child(Editor::new(&self.pre).h(px(84.))))
            .child(v_flex().gap_1().child(label("后置 post", "")).child(Editor::new(&self.post).h(px(84.))))
    }

    fn toggle(&mut self, which: &'static str, value: bool, cx: &mut Context<Self>) {
        match which {
            "skip" => self.skip = value,
            "insecure" => self.insecure = value,
            _ => self.follow_redirects = value,
        }
        self.refresh_dirty(cx);
        cx.notify();
    }

    fn render_advanced(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let label = |text: &'static str| div().text_xs().font_weight(FontWeight::MEDIUM).text_color(muted).child(text);
        let switch = |id: &'static str, which: &'static str, checked: bool, text: &'static str, cx: &mut Context<Self>| {
            Switch::new(id).small().label(text).checked(checked).on_change(cx.listener(move |this, v: &bool, _, cx| this.toggle(which, *v, cx)))
        };
        v_flex()
            .id("case-advanced")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_4()
            .gap_4()
            .child(v_flex().gap_1().child(label("标签")).child(Input::new(&self.tags).small()))
            .child(
                h_flex()
                    .gap_3()
                    .child(v_flex().w(px(200.)).gap_1().child(label(if self.protocol == Protocol::Ws { "连接与每次等待的超时 timeoutMs" } else { "超时 timeoutMs" })).child(Input::new(&self.timeout).small()))
                    .when(self.protocol == Protocol::Http, |this| this.child(v_flex().w(px(200.)).gap_1().child(label("传输层重试 retry")).child(Input::new(&self.retry).small()))),
            )
            .when(self.protocol == Protocol::Ws, |this| {
                this.child(v_flex().gap_1().child(label("子协议 subprotocols")).child(Input::new(&self.ws_subprotocols).small()))
            })
            .child(
                v_flex()
                    .gap_2()
                    .child(switch("case-skip", "skip", self.skip, "跳过该用例（保留在计划中，不执行）", cx))
                    .child(switch("case-insecure", "insecure", self.insecure, "跳过 TLS 证书校验（自签证书的测试环境）", cx))
                    .when(self.protocol == Protocol::Http, |this| this.child(switch("case-follow", "follow", self.follow_redirects, "跟随 3xx 重定向", cx))),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(label("数据驱动 cases：用例 × 每行数据 = 一个实例，行变量优先级最高"))
                    .child(
                        RadioGroup::horizontal("data-mode")
                            .children(["不使用", "内联数据", "数据文件"])
                            .selected_index(Some(self.data_mode))
                            .on_change(cx.listener(|this, ix: &usize, _, cx| {
                                this.data_mode = *ix;
                                this.refresh_dirty(cx);
                                cx.notify();
                            })),
                    )
                    .when(self.data_mode == 1, |this| this.child(Editor::new(&self.cases_inline).h(px(160.))))
                    .when(self.data_mode == 2, |this| this.child(Input::new(&self.cases_file).small())),
            )
    }

    fn render_vars(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let body = match &self.vars_preview {
            None => div().text_xs().text_color(muted).child("保存后计算").into_any_element(),
            Some(Err(e)) => div().text_xs().text_color(cx.theme().danger).child(e.clone()).into_any_element(),
            Some(Ok(p)) => {
                let mono = cx.theme().mono_font_family.clone();
                let used_color = cx.theme().primary;
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_3()
                            .px_2()
                            .text_xs()
                            .text_color(muted)
                            .child(div().w(px(14.)))
                            .child(div().w(px(160.)).child("变量"))
                            .child(div().flex_1().child("最终值"))
                            .child(div().w(px(200.)).child("来源（覆盖链）")),
                    )
                    .children(p.entries.iter().map(|e| {
                        let chain = if e.overridden.is_empty() {
                            e.source.clone()
                        } else {
                            format!("{} ← {}", e.source, e.overridden.iter().rev().cloned().collect::<Vec<_>>().join(" ← "))
                        };
                        h_flex()
                            .gap_3()
                            .px_2()
                            .py_1()
                            .text_xs()
                            .border_b_1()
                            .border_color(cx.theme().table_row_border)
                            .child(div().w(px(14.)).child(if e.used { Icon::new(IconName::Check).xsmall().text_color(used_color).into_any_element() } else { div().into_any_element() }))
                            .child(div().w(px(160.)).font_family(mono.clone()).child(e.name.clone()))
                            .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().font_family(mono.clone()).child(e.value.clone()))
                            .child(div().w(px(200.)).text_color(muted).child(chain))
                    }))
                    .when(!p.runtime.is_empty(), |this| {
                        this.child(div().pt_3().text_xs().font_weight(FontWeight::MEDIUM).child("运行时捕获（脚本写入 vars.x，按执行顺序在运行时生效）"))
                            .children(p.runtime.iter().map(|r| {
                                h_flex()
                                    .gap_3()
                                    .px_2()
                                    .py_1()
                                    .text_xs()
                                    .child(div().w(px(14.)).child(if r.used { Icon::new(IconName::Check).xsmall().text_color(used_color).into_any_element() } else { div().into_any_element() }))
                                    .child(div().w(px(160.)).font_family(mono.clone()).child(r.name.clone()))
                                    .child(div().flex_1().text_color(muted).child(format!("写入方：{}", r.writers.join(", "))))
                                    .when(r.shadows_static, |this| this.child(div().text_color(cx.theme().warning).child("运行时会覆盖同名静态变量")))
                            }))
                    })
                    .children(p.process_env.iter().map(|(n, set)| {
                        h_flex()
                            .gap_2()
                            .pt_2()
                            .text_xs()
                            .text_color(if *set { muted } else { cx.theme().warning })
                            .child(Icon::new(if *set { IconName::Check } else { IconName::TriangleAlert }).xsmall())
                            .child(format!("{{{{%{n}}}}} 进程环境变量{}", if *set { "已设置" } else { "未设置：运行时将报错" }))
                    }))
                    .children(p.undefined.iter().map(|u| {
                        h_flex()
                            .gap_2()
                            .pt_2()
                            .text_xs()
                            .text_color(cx.theme().danger)
                            .child(Icon::new(IconName::CircleX).xsmall())
                            .child(format!("{u} 被引用但没有任何一层定义"))
                    }))
                    .into_any_element()
            }
        };
        v_flex()
            .id("case-vars")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_4()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(muted)
                            .child(format!("本用例在环境 {} 下的变量解析（只读）· 打勾 = 本用例引用 · 优先级：项目 < 环境 < 集合（浅→深）< 数据驱动 < 运行时捕获", self.env)),
                    )
                    .child(
                        Button::new("open-vars-panel")
                            .small()
                            .outline()
                            .icon(IconName::Settings2)
                            .label("在变量面板中编辑 →")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(CaseEditorEvent::OpenVars))),
                    ),
            )
            .child(body)
    }

    /// 按写法建议改写当前编辑内容（与 `requestbaby fmt --fix` 同一套规则），改完留在编辑器里待保存
    fn apply_fixes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let doc = match self.collect(cx) {
            Ok(d) => d,
            Err(e) => {
                self.error = Some(e);
                cx.notify();
                return;
            }
        };
        let ctx = self
            .path
            .as_deref()
            .and_then(requestbaby_core::runner::discover::find_project_root)
            .map(|r| requestbaby_core::teach::Context::load(&r))
            .unwrap_or_default();
        let (fixed, advice) = requestbaby_core::teach::upgrade_case(&doc, &ctx);
        if !advice.iter().any(|a| a.fixable) {
            return;
        }
        self.base = fixed;
        self.load_form(window, cx);
        let text = fmt::canonical_case_text(&self.base);
        self.json.update(cx, |s, cx| s.set_value(text, window, cx));
        if self.tab != EditorTab::Json {
            self.tab = EditorTab::Expect;
        }
        self.refresh_dirty(cx);
        cx.notify();
    }

    fn render_issues(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let fixable = self.issues.iter().filter(|i| i.fixable).count();
        // 自动改写不了的写法建议：不逐条列给人，复制成修改指令交给 AI
        let for_ai: Vec<ValidationIssue> = self.issues.iter().filter(|i| !i.fixable).cloned().collect();
        let notice = style::style_notice("case-ai-brief", &for_ai, "", cx);
        v_flex()
            .px_4()
            .py_2()
            .gap_1()
            .border_t_1()
            .border_color(cx.theme().border)
            .when(fixable > 0, |this| {
                this.child(
                    h_flex()
                        .gap_2()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(div().flex_1().min_w_0().child(format!("{fixable} 条写法建议可以自动改写（语义不变，改完检查后保存）")))
                        .child(
                            Button::new("apply-fixes")
                                .xsmall()
                                .outline()
                                .icon(IconName::Replace)
                                .label("一键改写")
                                .tooltip("等价命令：requestbaby fmt --fix <用例文件>")
                                .on_click(cx.listener(|this, _, window, cx| this.apply_fixes(window, cx))),
                        ),
                )
            })
            .when_some(notice, |this, n| this.child(n))
            .when_some(self.error.clone(), |this, e| {
                this.child(
                    h_flex()
                        .gap_2()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(Icon::new(IconName::CircleX).xsmall())
                        .child(e),
                )
            })
            .children(self.issues.iter().filter(|i| !i.is_style()).enumerate().map(|(ix, i)| {
                let color = if i.level == "error" { cx.theme().danger } else { cx.theme().warning };
                h_flex()
                    .id(("issue", ix))
                    .items_start()
                    .gap_2()
                    .text_xs()
                    .child(
                        Icon::new(if i.level == "error" { IconName::CircleX } else { IconName::TriangleAlert })
                            .xsmall()
                            .text_color(color),
                    )
                    .child(div().flex_shrink_0().text_color(cx.theme().muted_foreground).child(format!("[{}]", i.rule)))
                    .child(div().flex_1().min_w_0().child(i.message.clone()))
            }))
    }
}

impl Render for CaseEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.rel.is_none() {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .text_color(cx.theme().muted_foreground)
                .child(Icon::new(IconName::FileText).large())
                .child(Label::new("在左侧选择一个用例"))
                .child(div().text_xs().child("用例即文件：这里的每次保存都等同于 AI 或你在编辑器里改了 JSON"))
                .into_any_element();
        }
        let selected = EditorTab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
        let has_footer = self.error.is_some() || !self.issues.is_empty();
        v_flex()
            .size_full()
            .child(self.render_header(cx))
            .child(
                h_flex().px_4().pt_2().child(
                    TabBar::new("case-editor-tabs")
                        .segmented()
                        .small()
                        .selected_index(selected)
                        .children(EditorTab::ALL.iter().map(|t| Tab::new().label(t.label())))
                        .on_click(cx.listener(|this, ix: &usize, window, cx| {
                            this.set_tab(EditorTab::ALL[*ix], window, cx);
                        })),
                ),
            )
            .map(|this| match self.tab {
                EditorTab::Request => this.child(self.render_request(cx)),
                EditorTab::Expect => this.child(self.render_expect(cx)),
                EditorTab::Scripts => this.child(self.render_scripts(cx)),
                EditorTab::Advanced => this.child(self.render_advanced(cx)),
                EditorTab::Vars => this.child(self.render_vars(cx)),
                EditorTab::Json => this.child(
                    div().flex_1().min_h_0().p_4().child(Editor::new(&self.json).h_full()),
                ),
            })
            .when(has_footer, |this| this.child(self.render_issues(cx)))
            .into_any_element()
    }
}
