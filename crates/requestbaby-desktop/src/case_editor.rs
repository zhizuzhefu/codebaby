//! 用例编辑器：表单 / 脚本 / JSON 三个视图编辑同一个用例文件。
//!
//! 文件是唯一事实：表单只覆盖它认识的字段，其余字段（skip、cases、timeoutMs…）原样保留；
//! 保存走 core 的规范化格式（与 `requestbaby fmt` 字节一致），随后立即跑 `validate`。
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, IndexPath, Sizable as _, h_flex,
    button::{Button, ButtonVariants as _},
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
    App, AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
};
use requestbaby_core::{fmt, inspect::VarsPreview, validate::ValidationIssue};
use serde_json::{Map, Value, json};
use std::path::PathBuf;

const METHODS: [&str; 7] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorTab {
    Request,
    Scripts,
    Advanced,
    Vars,
    Json,
}

impl EditorTab {
    const ALL: [EditorTab; 5] = [EditorTab::Request, EditorTab::Scripts, EditorTab::Advanced, EditorTab::Vars, EditorTab::Json];

    fn label(self) -> &'static str {
        match self {
            EditorTab::Request => "请求",
            EditorTab::Scripts => "脚本与断言",
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
}

pub struct CaseEditor {
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
    inherited_headers: Vec<String>,
    error: Option<String>,

    name: Entity<InputState>,
    method: Entity<SelectState<Vec<SharedString>>>,
    url: Entity<InputState>,
    expect_status: Entity<InputState>,
    headers: Entity<EditorState>,
    body: Entity<EditorState>,
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
        let expect_status = cx.new(|cx| InputState::new(window, cx).placeholder("如 200 或 200, 201"));
        let methods: Vec<SharedString> = METHODS.iter().map(|m| SharedString::from(*m)).collect();
        let method = cx.new(|cx| SelectState::new(methods, Some(IndexPath::new(0)), window, cx));
        let headers = cx.new(|cx| code_editor("text", "Header-Name: value（每行一个）", window, cx));
        let body = cx.new(|cx| code_editor("json", "JSON 请求体；非 JSON 文本按原样发送", window, cx));
        let tests = cx.new(|cx| code_editor("javascript", "每行一条断言，如 data.code === 0", window, cx));
        let pre = cx.new(|cx| code_editor("javascript", "请求前执行，可改写 req", window, cx));
        let post = cx.new(|cx| code_editor("javascript", "响应后执行，如 vars.token = res.json('token')", window, cx));
        let json = cx.new(|cx| code_editor("json", "", window, cx));
        let tags = cx.new(|cx| InputState::new(window, cx).placeholder("smoke, auth（逗号分隔，与集合标签合并）"));
        let timeout = cx.new(|cx| InputState::new(window, cx).placeholder("缺省用项目 timeoutMs"));
        let retry = cx.new(|cx| InputState::new(window, cx).placeholder("0"));
        let cases_inline = cx.new(|cx| code_editor("json", "[{\"label\": \"正常\", \"sku\": \"A1\"}]", window, cx));
        let cases_file = cx.new(|cx| InputState::new(window, cx).placeholder("./data/users.csv（相对用例目录；CSV 首行表头或 JSON 数组）"));

        let mut subs = vec![];
        for input in [&name, &url, &expect_status, &tags, &timeout, &retry, &cases_file] {
            subs.push(cx.subscribe_in(input, window, |this, _, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.refresh_dirty(cx);
                }
            }));
        }
        for editor in [&headers, &body, &tests, &pre, &post, &json, &cases_inline] {
            subs.push(cx.subscribe_in(editor, window, |this, _, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.refresh_dirty(cx);
                }
            }));
        }
        subs.push(cx.subscribe_in(&method, window, |this, _, _: &SelectEvent<Vec<SharedString>>, _, cx| {
            this.refresh_dirty(cx);
        }));

        Self {
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
            method,
            url,
            expect_status,
            headers,
            body,
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

    pub fn set_env(&mut self, env: String) {
        self.env = env;
        self.validate_now();
    }

    /// 打开用例文件（未保存的修改由调用方先确认）
    pub fn open(&mut self, rel: String, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        self.inherited_headers = requestbaby_core::runner::discover::discover(std::slice::from_ref(&path))
            .ok()
            .and_then(|v| v.into_iter().next())
            .map(|dc| {
                let mut keys: Vec<String> = vec![];
                for c in &dc.chain {
                    for k in c.headers.keys() {
                        if !keys.iter().any(|x| x.eq_ignore_ascii_case(k)) {
                            keys.push(k.clone());
                        }
                    }
                }
                keys
            })
            .unwrap_or_default();
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
        self.validate_now();
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
        let d = self.base.clone();
        let s = |v: Option<&Value>| v.and_then(|x| x.as_str()).unwrap_or_default().to_string();
        let name = s(d.get("name"));
        let url = s(d.pointer("/request/url"));
        let method = s(d.pointer("/request/method")).to_uppercase();
        let expect = match d.get("expectStatus") {
            Some(Value::Number(n)) => n.to_string(),
            Some(Value::Array(a)) => a.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", "),
            _ => String::new(),
        };
        let headers = d
            .pointer("/request/headers")
            .and_then(|h| h.as_object())
            .map(|h| h.iter().map(|(k, v)| format!("{k}: {}", v.as_str().unwrap_or_default())).collect::<Vec<_>>().join("\n"))
            .unwrap_or_default();
        let body = match d.pointer("/request/body") {
            Some(Value::Object(o)) if o.contains_key("json") => serde_json::to_string_pretty(&o["json"]).unwrap_or_default(),
            Some(Value::Object(o)) => o.get("text").and_then(|t| t.as_str()).unwrap_or_default().to_string(),
            Some(Value::String(t)) => t.clone(),
            _ => String::new(),
        };
        let tests = d
            .get("tests")
            .and_then(|t| t.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join("\n"))
            .unwrap_or_default();
        let pre = s(d.get("pre"));
        let post = s(d.get("post"));

        self.name.update(cx, |st, cx| st.set_value(name, window, cx));
        self.url.update(cx, |st, cx| st.set_value(url, window, cx));
        self.expect_status.update(cx, |st, cx| st.set_value(expect, window, cx));
        self.headers.update(cx, |st, cx| st.set_value(headers, window, cx));
        self.body.update(cx, |st, cx| st.set_value(body, window, cx));
        self.tests.update(cx, |st, cx| st.set_value(tests, window, cx));
        self.pre.update(cx, |st, cx| st.set_value(pre, window, cx));
        self.post.update(cx, |st, cx| st.set_value(post, window, cx));
        let tags = d
            .get("tags")
            .and_then(|t| t.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "))
            .unwrap_or_default();
        let num = |v: Option<&Value>| v.map(|x| x.to_string()).unwrap_or_default();
        let timeout = num(d.pointer("/request/timeoutMs"));
        let retry = num(d.pointer("/request/retry"));
        self.skip = d.get("skip").and_then(|v| v.as_bool()).unwrap_or(false);
        self.insecure = d.pointer("/request/insecure").and_then(|v| v.as_bool()).unwrap_or(false);
        self.follow_redirects = d.pointer("/request/followRedirects").and_then(|v| v.as_bool()).unwrap_or(false);
        let (mode, inline, file) = match d.get("cases") {
            Some(Value::Array(a)) => (1, serde_json::to_string_pretty(a).unwrap_or_default(), String::new()),
            Some(Value::Object(o)) => (2, String::new(), o.get("file").and_then(|f| f.as_str()).unwrap_or_default().to_string()),
            _ => (0, String::new(), String::new()),
        };
        self.data_mode = mode;
        self.tags.update(cx, |st, cx| st.set_value(tags, window, cx));
        self.timeout.update(cx, |st, cx| st.set_value(timeout, window, cx));
        self.retry.update(cx, |st, cx| st.set_value(retry, window, cx));
        self.cases_inline.update(cx, |st, cx| st.set_value(inline, window, cx));
        self.cases_file.update(cx, |st, cx| st.set_value(file, window, cx));
        let method: SharedString = if method.is_empty() { "GET".into() } else { method.into() };
        self.method.update(cx, |st, cx| st.set_selected_value(&method, window, cx));
    }

    /// 当前编辑内容 → 文档（JSON 视图以文本为准，其余视图以表单覆盖 base）
    fn collect(&self, cx: &App) -> Result<Value, String> {
        if self.tab == EditorTab::Json {
            return serde_json::from_str::<Value>(&self.json.read(cx).value()).map_err(|e| format!("JSON 解析失败: {e}"));
        }
        let mut d = match &self.base {
            Value::Object(_) => self.base.clone(),
            _ => json!({}),
        };
        let obj = d.as_object_mut().expect("object");
        let text = |e: &Entity<EditorState>| e.read(cx).value().to_string();
        let set_or_remove = |obj: &mut Map<String, Value>, key: &str, v: Option<Value>| match v {
            Some(v) => {
                obj.insert(key.into(), v);
            }
            None => {
                obj.remove(key);
            }
        };

        let name = self.name.read(cx).value().trim().to_string();
        set_or_remove(obj, "name", (!name.is_empty()).then(|| Value::String(name)));
        let tags: Vec<Value> = self
            .tags
            .read(cx)
            .value()
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| Value::String(t.to_string()))
            .collect();
        set_or_remove(obj, "tags", (!tags.is_empty()).then(|| Value::Array(tags)));
        set_or_remove(obj, "skip", self.skip.then_some(Value::Bool(true)));
        let cases = match self.data_mode {
            1 => {
                let raw = text(&self.cases_inline);
                if raw.trim().is_empty() {
                    None
                } else {
                    let v: Value = serde_json::from_str(&raw).map_err(|e| format!("数据驱动（内联）JSON 解析失败: {e}"))?;
                    if !v.is_array() {
                        return Err("数据驱动（内联）必须是对象数组".into());
                    }
                    Some(v)
                }
            }
            2 => {
                let f = self.cases_file.read(cx).value().trim().to_string();
                (!f.is_empty()).then(|| json!({ "file": f }))
            }
            _ => None,
        };
        set_or_remove(obj, "cases", cases);

        let expect = self.expect_status.read(cx).value().to_string();
        let codes: Vec<u16> = expect.split([',', ' ']).filter_map(|x| x.trim().parse().ok()).collect();
        let expect_v = match codes.len() {
            0 => None,
            1 => Some(json!(codes[0])),
            _ => Some(json!(codes)),
        };
        set_or_remove(obj, "expectStatus", expect_v);

        let tests: Vec<Value> = text(&self.tests)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|l| Value::String(l.to_string()))
            .collect();
        set_or_remove(obj, "tests", (!tests.is_empty()).then(|| Value::Array(tests)));
        let pre = text(&self.pre);
        set_or_remove(obj, "pre", (!pre.trim().is_empty()).then(|| Value::String(pre.trim_end().to_string())));
        let post = text(&self.post);
        set_or_remove(obj, "post", (!post.trim().is_empty()).then(|| Value::String(post.trim_end().to_string())));

        let req = obj.entry("request").or_insert_with(|| json!({}));
        if !req.is_object() {
            *req = json!({});
        }
        let req = req.as_object_mut().expect("object");
        let method = self.method.read(cx).selected_value().cloned().unwrap_or_else(|| "GET".into());
        req.insert("method".into(), Value::String(method.to_string()));
        req.insert("url".into(), Value::String(self.url.read(cx).value().trim().to_string()));

        let mut headers = Map::new();
        for line in text(&self.headers).lines() {
            if let Some((k, v)) = line.split_once(':') {
                if !k.trim().is_empty() {
                    headers.insert(k.trim().to_string(), Value::String(v.trim().to_string()));
                }
            }
        }
        set_or_remove(req, "headers", (!headers.is_empty()).then(|| Value::Object(headers)));

        let body = text(&self.body);
        let body_v = if body.trim().is_empty() {
            None
        } else {
            match serde_json::from_str::<Value>(&body) {
                Ok(v) => Some(json!({ "json": v })),
                Err(_) => Some(json!({ "text": body })),
            }
        };
        set_or_remove(req, "body", body_v);
        let number = |e: &Entity<InputState>| -> Result<Option<Value>, String> {
            let t = e.read(cx).value().trim().to_string();
            if t.is_empty() {
                return Ok(None);
            }
            t.parse::<u64>().map(|n| Some(json!(n))).map_err(|_| format!("「{t}」不是非负整数"))
        };
        set_or_remove(req, "timeoutMs", number(&self.timeout)?);
        set_or_remove(req, "retry", number(&self.retry)?);
        set_or_remove(req, "insecure", self.insecure.then_some(Value::Bool(true)));
        set_or_remove(req, "followRedirects", self.follow_redirects.then_some(Value::Bool(true)));
        Ok(d)
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
        if let Err(e) = std::fs::write(&path, &text) {
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
        self.validate_now();
        cx.emit(CaseEditorEvent::Saved);
        cx.notify();
        true
    }

    /// 校验（全项目上下文，跨用例捕获的变量不误报）+ 变量解析预览
    fn validate_now(&mut self) {
        let Some(path) = &self.path else { return };
        let env = if self.env.is_empty() { "dev".to_string() } else { self.env.clone() };
        self.issues = requestbaby_core::validate::validate_file(path, &env).unwrap_or_default();
        self.vars_preview = Some(requestbaby_core::inspect::case_vars(path, &env).map_err(|e| format!("{e:#}")));
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rel = self.rel.clone().unwrap_or_default();
        let errors = self.issues.iter().filter(|i| i.level == "error").count();
        let warns = self.issues.len() - errors;
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
                            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(rel.clone()))
                            .when(self.dirty, |this| {
                                this.child(Tag::warning().outline().xsmall().child("未保存"))
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("requestbaby send {rel} -e {}", self.env)),
                    ),
            )
            .child(
                h_flex()
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
        let label = |text: &'static str| {
            div().text_xs().font_weight(FontWeight::MEDIUM).text_color(cx.theme().muted_foreground).child(text)
        };
        v_flex()
            .flex_1()
            .min_h_0()
            .p_4()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .child(div().w(px(116.)).child(Select::new(&self.method).small()))
                    .child(div().flex_1().child(Input::new(&self.url).small())),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(v_flex().flex_1().gap_1().child(label("名称")).child(Input::new(&self.name).small()))
                    .child(v_flex().w(px(180.)).gap_1().child(label("期望状态码")).child(Input::new(&self.expect_status).small())),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(label("请求头"))
                            .when(!self.inherited_headers.is_empty(), |this| {
                                this.child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(format!("另继承自集合：{}", self.inherited_headers.join(", "))),
                                )
                            }),
                    )
                    .child(Editor::new(&self.headers).h(px(84.))),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_1()
                    .child(label("请求体"))
                    .child(div().flex_1().min_h_0().child(Editor::new(&self.body).h_full())),
            )
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
                    .child(label("断言 tests", "每行一条 JS 表达式；可用 res / data / vars / env"))
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
                    .child(v_flex().w(px(200.)).gap_1().child(label("超时 timeoutMs")).child(Input::new(&self.timeout).small()))
                    .child(v_flex().w(px(200.)).gap_1().child(label("传输层重试 retry")).child(Input::new(&self.retry).small())),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(switch("case-skip", "skip", self.skip, "跳过该用例（保留在计划中，不执行）", cx))
                    .child(switch("case-insecure", "insecure", self.insecure, "跳过 TLS 证书校验", cx))
                    .child(switch("case-follow", "follow", self.follow_redirects, "跟随 3xx 重定向", cx)),
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
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(format!("环境 {} · 优先级：项目 < 环境 < 集合（浅→深）< 数据驱动 < 运行时捕获 · 打勾 = 本用例引用", self.env)),
            )
            .child(body)
    }

    fn render_issues(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .px_4()
            .py_2()
            .gap_1()
            .border_t_1()
            .border_color(cx.theme().border)
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
            .children(self.issues.iter().enumerate().map(|(ix, i)| {
                let color = if i.level == "error" { cx.theme().danger } else { cx.theme().warning };
                h_flex()
                    .id(("issue", ix))
                    .gap_2()
                    .text_xs()
                    .child(
                        Icon::new(if i.level == "error" { IconName::CircleX } else { IconName::TriangleAlert })
                            .xsmall()
                            .text_color(color),
                    )
                    .child(div().text_color(cx.theme().muted_foreground).child(format!("[{}]", i.rule)))
                    .child(i.message.clone())
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
                div().px_4().pt_2().child(
                    TabBar::new("case-editor-tabs")
                        .underline()
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
