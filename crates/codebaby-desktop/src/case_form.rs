//! 用例编辑器的表单模型：文件 ↔ 表单的映射（纯数据，可测试）。
//!
//! 写回原则：只改用户改动过的字段。保存时把当前表单与「打开时的表单」逐字段比较，
//! 没变的字段原样保留文件里的写法（多行 JS 断言、末尾换行、`[200]` 这样的写法都不会被改写）。
//! 协议（request / grpc / ws / web / android / desktop）切换时，换掉请求部分、其余（断言、捕获、脚本…）保留。
use codebaby_core::config::{Locator, Protocol, UiAction, UiKind, UiStep, WsAction};
use serde_json::{Map, Value, json};

/// gRPC 调用
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GrpcForm {
    pub target: String,
    pub method: String,
    /// 消息 JSON 文本：单条时是对象，streaming 时是数组
    pub message: String,
    /// 多条消息（客户端流 / 双向流，写 messages）
    pub streaming: bool,
    pub metadata: Map<String, Value>,
}

/// WebSocket 对话
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WsForm {
    pub url: String,
    pub headers: Map<String, Value>,
    pub query: Map<String, Value>,
    /// 逗号分隔
    pub subprotocols: String,
    pub steps: Vec<StepForm>,
}

/// 一个对话步骤：动作 + 一行内容（发送的消息 / 接收条件 JSON / 毫秒数 / 关闭码）
#[derive(Debug, Clone, PartialEq)]
pub struct StepForm {
    pub action: WsAction,
    pub content: String,
    /// `变量=路径`，逗号分隔（只用于接收 / 等到）
    pub capture: String,
    pub timeout: String,
    /// 打开时的原样步骤：内容没改就原样写回（保留数字、布尔这类写法）
    pub raw: Option<Value>,
}

impl StepForm {
    pub fn new(action: WsAction) -> Self {
        let content = match action {
            WsAction::Receive | WsAction::WaitFor => "{}".to_string(),
            WsAction::Wait => "500".to_string(),
            _ => String::new(),
        };
        StepForm { action, content, capture: String::new(), timeout: String::new(), raw: None }
    }

    fn from_value(v: &Value) -> StepForm {
        let o = v.as_object().cloned().unwrap_or_default();
        let step: codebaby_core::config::WsStep = serde_json::from_value(v.clone()).unwrap_or_default();
        let action = step.action().or_else(|| step.actions().first().copied()).unwrap_or(WsAction::Send);
        let compact = |x: &Value| serde_json::to_string(x).unwrap_or_default();
        let content = match action {
            WsAction::Send => match o.get("send") {
                Some(Value::String(s)) => s.clone(),
                Some(x) => compact(x),
                None => String::new(),
            },
            WsAction::SendBinary => text(o.get("sendBinary")),
            WsAction::Receive => o.get("receive").map(compact).unwrap_or_default(),
            WsAction::WaitFor => o.get("waitFor").map(compact).unwrap_or_default(),
            WsAction::Wait => o.get("wait").map(|x| x.to_string()).unwrap_or_default(),
            WsAction::Close => match o.get("close") {
                Some(Value::Number(n)) => n.to_string(),
                _ => String::new(),
            },
        };
        let capture = o
            .get("capture")
            .and_then(Value::as_object)
            .map(|m| m.iter().map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or_default())).collect::<Vec<_>>().join(", "))
            .unwrap_or_default();
        let timeout = o.get("timeoutMs").map(|x| x.to_string()).unwrap_or_default();
        StepForm { action, content, capture, timeout, raw: Some(v.clone()) }
    }

    /// 写回：与打开时相同就原样保留
    fn to_value(&self, n: usize) -> Result<Value, String> {
        if let Some(raw) = &self.raw {
            if StepForm::from_value(raw) == (StepForm { raw: Some(raw.clone()), ..self.clone() }) {
                return Ok(raw.clone());
            }
        }
        let c = self.content.trim();
        let mut o = Map::new();
        match self.action {
            WsAction::Send => {
                // 对象 / 数组按 JSON 发送，其余原样作为文本
                let v = match serde_json::from_str::<Value>(c) {
                    Ok(v @ (Value::Object(_) | Value::Array(_))) => v,
                    _ => Value::String(self.content.clone()),
                };
                o.insert("send".into(), v);
            }
            WsAction::SendBinary => {
                o.insert("sendBinary".into(), Value::String(c.into()));
            }
            WsAction::Receive | WsAction::WaitFor => {
                let v = if c.is_empty() { json!({}) } else { serde_json::from_str::<Value>(c).map_err(|e| format!("第 {n} 步的条件不是合法 JSON：{e}"))? };
                if !v.is_object() {
                    return Err(format!("第 {n} 步的条件要写成对象：{{\"type\": \"joined\"}}，整条文本消息写 {{\"$\": \"pong\"}}"));
                }
                o.insert(self.action.key().into(), v);
            }
            WsAction::Wait => {
                let ms: u64 = c.parse().map_err(|_| format!("第 {n} 步的静候时长「{c}」不是毫秒数"))?;
                o.insert("wait".into(), json!(ms));
            }
            WsAction::Close => {
                let v = if c.is_empty() { Value::Bool(true) } else { json!(c.parse::<u16>().map_err(|_| format!("第 {n} 步的关闭码「{c}」不是数字"))?) };
                o.insert("close".into(), v);
            }
        }
        let mut cap = Map::new();
        for pair in self.capture.split([',', '，']).map(str::trim).filter(|p| !p.is_empty()) {
            let (k, v) = pair.split_once('=').ok_or_else(|| format!("第 {n} 步的捕获「{pair}」要写成 变量名=路径"))?;
            cap.insert(k.trim().into(), Value::String(v.trim().into()));
        }
        if !cap.is_empty() {
            o.insert("capture".into(), Value::Object(cap));
        }
        let t = self.timeout.trim();
        if !t.is_empty() {
            o.insert("timeoutMs".into(), json!(t.parse::<u64>().map_err(|_| format!("第 {n} 步的超时「{t}」不是毫秒数"))?));
        }
        Ok(Value::Object(o))
    }
}

/// 界面用例（Web / Android / Mac 桌面）：打开的页面 / 启动的应用 + 步骤
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UiForm {
    /// Web：url；Android：app（包名）；Mac 桌面：app（名称或 bundle ID）
    pub target: String,
    /// Mac 桌面：窗口标题包含的文字
    pub window: String,
    /// Mac 桌面：开始前先退出再启动
    pub restart: bool,
    /// Android：启动的 Activity
    pub activity: String,
    /// Android：先安装的 APK
    pub apk: String,
    /// Android：启动前清除数据
    pub clear_data: bool,
    /// Android：接管设备当前的界面（不启动、不关闭应用）
    pub attach: bool,
    pub steps: Vec<UiStepForm>,
}

/// 界面步骤：动作 + 目标（元素定位 JSON；打开 / 按键 / 输入 / 滑动 / 截图 / 静候是一行文字）+ 值
#[derive(Debug, Clone, PartialEq)]
pub struct UiStepForm {
    pub action: UiAction,
    /// 元素定位：`{"role": "button", "name": "登录"}`；直接写文字 = `{"text": 文字}`
    pub target: String,
    /// fill / select 的值
    pub value: String,
    /// 捕获：JSON 对象 `{"orderNo": {"testId": "order-no"}, "page": "url"}`
    pub capture: String,
    pub timeout: String,
    pub optional: bool,
    /// launch 前清除数据
    pub clear_data: bool,
    /// 打开时的原样步骤：没改就原样写回
    pub raw: Option<Value>,
}

/// 动作的目标是元素定位（其余动作是一行文字或没有内容）
pub fn ui_targets_locator(a: UiAction) -> bool {
    a.targets_element() || matches!(a, UiAction::WaitFor | UiAction::Expect)
}

/// 动作带一行文字内容（repeat / if 是 JSON 对象文本，notify 是通知文字或 JSON 对象文本，wait 是毫秒数或 `{"min", "max"}`）
pub fn ui_has_text(a: UiAction) -> bool {
    matches!(a, UiAction::Goto | UiAction::Type | UiAction::Press | UiAction::Swipe | UiAction::Screenshot | UiAction::Wait | UiAction::Repeat | UiAction::If | UiAction::Notify)
}

/// 目标是一整块 JSON 对象的动作：repeat / if
pub fn ui_is_block(a: UiAction) -> bool {
    matches!(a, UiAction::Repeat | UiAction::If)
}

/// 定位输入框的文字 → 定位：JSON 对象原样；其余当作可见文字
pub fn parse_locator(t: &str) -> Result<Value, String> {
    let t = t.trim();
    if t.is_empty() {
        return Err("要写目标元素".into());
    }
    if t.starts_with('{') {
        let v: Value = serde_json::from_str(t).map_err(|e| format!("定位不是合法 JSON：{e}"))?;
        serde_json::from_value::<Locator>(v.clone()).map_err(|e| format!("定位写法不对：{e}"))?;
        return Ok(v);
    }
    Ok(json!({ "text": t }))
}

impl UiStepForm {
    pub fn new(action: UiAction) -> Self {
        let target = match action {
            UiAction::Wait => "500".to_string(),
            UiAction::Repeat => r#"{"times": 10, "steps": [{"wait": {"min": 3000, "max": 8000}}]}"#.to_string(),
            UiAction::If => r#"{"when": {"text": "要判断的文字"}, "then": [{"wait": 1000}]}"#.to_string(),
            UiAction::Swipe => "up".to_string(),
            UiAction::Press => "Enter".to_string(),
            _ => String::new(),
        };
        UiStepForm { action, target, value: String::new(), capture: String::new(), timeout: String::new(), optional: false, clear_data: false, raw: None }
    }

    pub fn from_value(v: &Value, kind: UiKind) -> UiStepForm {
        let o = v.as_object().cloned().unwrap_or_default();
        let step: UiStep = serde_json::from_value(v.clone()).unwrap_or_default();
        let action = step.action().or_else(|| step.actions().first().copied()).unwrap_or(UiAction::Click);
        let compact = |x: &Value| serde_json::to_string(x).unwrap_or_default();
        let target = if ui_targets_locator(action) {
            let key = action.key(kind);
            // 点击：Web / Mac 桌面写 click、Android 写 tap，两种写法都认
            o.get(key).or_else(|| o.get(if key == "tap" { "click" } else if key == "click" { "tap" } else { key })).map(compact).unwrap_or_default()
        } else {
            match action {
                // 毫秒数或 {"min", "max"}；repeat / if 是整个对象；notify 是文字，带 webhook 时是对象
                UiAction::Wait | UiAction::Repeat | UiAction::If | UiAction::Notify => match o.get(action.key(kind)) {
                    Some(Value::String(t)) => t.clone(),
                    Some(x) => compact(x),
                    None => String::new(),
                },
                a => o.get(a.key(kind)).and_then(Value::as_str).unwrap_or_default().to_string(),
            }
        };
        UiStepForm {
            action,
            target,
            value: text(o.get("value")),
            capture: o.get("capture").map(compact).unwrap_or_default(),
            // timeoutMs 写毫秒数；timeout 可以写人话（"15s"）
            timeout: match (o.get("timeoutMs"), o.get("timeout")) {
                (Some(x), _) => x.to_string(),
                (None, Some(Value::String(t))) => t.clone(),
                (None, Some(x)) => x.to_string(),
                _ => String::new(),
            },
            optional: o.get("optional").and_then(Value::as_bool).unwrap_or(false),
            clear_data: o.get("clearData").and_then(Value::as_bool).unwrap_or(false),
            raw: Some(v.clone()),
        }
    }

    /// 写回：与打开时相同就原样保留
    pub fn to_value(&self, n: usize, kind: UiKind) -> Result<Value, String> {
        if let Some(raw) = &self.raw {
            if UiStepForm::from_value(raw, kind) == (UiStepForm { raw: Some(raw.clone()), ..self.clone() }) {
                return Ok(raw.clone());
            }
        }
        let a = self.action;
        let t = self.target.trim();
        let mut o = Map::new();
        if ui_targets_locator(a) {
            let loc = parse_locator(t).map_err(|e| format!("第 {n} 步：{e}"))?;
            o.insert(a.key(kind).into(), loc);
        } else if a == UiAction::Wait {
            if t.starts_with('{') {
                let v: Value = serde_json::from_str(t).map_err(|e| format!("第 {n} 步的静候不是合法 JSON：{e}（写毫秒数，或 {{\"min\": 3000, \"max\": 8000}}）"))?;
                let w = serde_json::from_value::<codebaby_core::config::Wait>(v.clone()).map_err(|e| format!("第 {n} 步的静候写法不对：{e}"))?;
                w.check().map_err(|e| format!("第 {n} 步：{e}"))?;
                o.insert("wait".into(), v);
            } else if let Ok(ms) = t.parse::<u64>() {
                o.insert("wait".into(), json!(ms));
            } else {
                // 人话：「2.5s」「3s-8s」（原样保留用户写的文字）
                let w = serde_json::from_value::<codebaby_core::config::Wait>(Value::String(t.into())).map_err(|e| format!("第 {n} 步的静候「{t}」写法不对：{e}"))?;
                w.check().map_err(|e| format!("第 {n} 步：{e}"))?;
                o.insert("wait".into(), Value::String(t.into()));
            }
        } else if ui_is_block(a) {
            let v: Value = serde_json::from_str(t).map_err(|e| format!("第 {n} 步（{}）不是合法 JSON：{e}", a.label()))?;
            let ok = match a {
                UiAction::Repeat => serde_json::from_value::<codebaby_core::config::Repeat>(v.clone()).map(|_| ()),
                _ => serde_json::from_value::<codebaby_core::config::IfBlock>(v.clone()).map(|_| ()),
            };
            ok.map_err(|e| format!("第 {n} 步（{}）写法不对：{e}", a.label()))?;
            o.insert(a.key(kind).into(), v);
        } else if a == UiAction::Notify {
            if t.is_empty() {
                return Err(format!("第 {n} 步（通知）没有写内容"));
            }
            if t.starts_with('{') {
                let v: Value = serde_json::from_str(t).map_err(|e| format!("第 {n} 步的通知不是合法 JSON：{e}（直接写通知文字，或 {{\"message\": \"…\", \"webhook\": \"https://…\"}}）"))?;
                serde_json::from_value::<codebaby_core::config::NotifySpec>(v.clone()).map_err(|e| format!("第 {n} 步的通知写法不对：{e}"))?;
                o.insert("notify".into(), v);
            } else {
                o.insert("notify".into(), Value::String(t.into()));
            }
        } else if ui_has_text(a) {
            if t.is_empty() {
                return Err(format!("第 {n} 步（{}）没有写内容", a.label()));
            }
            o.insert(a.key(kind).into(), Value::String(t.into()));
        } else {
            o.insert(a.key(kind).into(), Value::Bool(true));
        }
        if matches!(a, UiAction::Fill | UiAction::Select) {
            o.insert("value".into(), Value::String(self.value.clone()));
        }
        if a == UiAction::Launch && self.clear_data {
            o.insert("clearData".into(), Value::Bool(true));
        }
        let c = self.capture.trim();
        if !c.is_empty() {
            let v: Value = serde_json::from_str(c).map_err(|e| format!("第 {n} 步的捕获不是合法 JSON：{e}（写成 {{\"变量名\": \"url\"}} 或 {{\"变量名\": 定位}}）"))?;
            if !v.is_object() {
                return Err(format!("第 {n} 步的捕获要写成对象：{{\"变量名\": \"url\"}}"));
            }
            o.insert("capture".into(), v);
        }
        let tm = self.timeout.trim();
        if !tm.is_empty() {
            match tm.parse::<u64>() {
                Ok(ms) => {
                    o.insert("timeoutMs".into(), json!(ms));
                }
                Err(_) => {
                    codebaby_core::config::ui::parse_duration_ms(tm).map_err(|e| format!("第 {n} 步的超时：{e}"))?;
                    o.insert("timeout".into(), Value::String(tm.into()));
                }
            }
        }
        if self.optional {
            o.insert("optional".into(), Value::Bool(true));
        }
        Ok(Value::Object(o))
    }
}

/// 请求体：模式 0 无 / 1 raw / 2 表单 / 3 文件上传；raw 时 raw_json 区分 JSON 与文本
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BodyForm {
    pub mode: usize,
    pub raw_json: bool,
    pub text: String,
    pub form: Map<String, Value>,
    pub multipart: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CaseForm {
    pub name: String,
    /// 由文件里写的是 request / grpc / ws 决定；编辑器顶部可切换
    pub protocol: Protocol,
    pub grpc: GrpcForm,
    pub ws: WsForm,
    /// Web / Android / Mac 桌面（按 protocol 写进 web / android / desktop）
    pub ui: UiForm,
    pub method: String,
    pub url: String,
    /// 状态码文本：`200` / `200, 201` / `{{status}}`
    pub status: String,
    /// 文件里状态码是旧写法 expectStatus：改动时原位写回
    pub legacy_status: bool,
    pub headers: Map<String, Value>,
    pub query: Map<String, Value>,
    pub body: BodyForm,
    pub expect_headers: Map<String, Value>,
    pub expect_body: Map<String, Value>,
    pub capture: Map<String, Value>,
    /// 每行一条
    pub tests: String,
    pub pre: String,
    pub post: String,
    /// 逗号分隔
    pub tags: String,
    /// 超时 / 跳过证书校验：写在当前协议的请求部分（request / grpc / ws）里
    pub timeout: String,
    /// 重试 / 跟随重定向只有 HTTP 有
    pub retry: String,
    pub skip: bool,
    pub insecure: bool,
    pub follow_redirects: bool,
    /// 数据驱动：0 无 / 1 内联 / 2 文件
    pub data_mode: usize,
    pub cases_inline: String,
    pub cases_file: String,
}

/// 界面协议对应的驱动种类（非界面协议按 Web 处理：界面表单只在界面协议下使用）
pub fn ui_kind(p: Protocol) -> UiKind {
    p.ui_kind().unwrap_or(UiKind::Web)
}

fn text(v: Option<&Value>) -> String {
    v.and_then(Value::as_str).unwrap_or_default().to_string()
}

fn object(d: &Value, ptr: &str) -> Map<String, Value> {
    d.pointer(ptr).and_then(Value::as_object).cloned().unwrap_or_default()
}

fn set(obj: &mut Map<String, Value>, key: &str, v: Option<Value>) {
    match v {
        Some(v) => {
            obj.insert(key.into(), v);
        }
        None => {
            obj.remove(key);
        }
    }
}

impl CaseForm {
    pub fn from_doc(d: &Value) -> CaseForm {
        let status = d.pointer("/expect/status").filter(|v| !v.is_null());
        let legacy_status = status.is_none() && d.get("expectStatus").is_some();
        let status = match status.or_else(|| d.get("expectStatus")) {
            Some(Value::Number(n)) => n.to_string(),
            Some(Value::Array(a)) => a.iter().map(|x| x.as_str().map(String::from).unwrap_or_else(|| x.to_string())).collect::<Vec<_>>().join(", "),
            Some(Value::String(v)) => v.clone(),
            _ => String::new(),
        };
        let (mode, raw_json, body_text) = match d.pointer("/request/body") {
            Some(Value::Object(o)) if o.contains_key("json") => (1, true, serde_json::to_string_pretty(&o["json"]).unwrap_or_default()),
            Some(Value::Object(o)) if o.contains_key("text") => (1, false, text(o.get("text"))),
            Some(Value::Object(o)) if o.contains_key("form") => (2, true, String::new()),
            Some(Value::Object(o)) if o.contains_key("multipart") => (3, true, String::new()),
            _ => (0, true, String::new()),
        };
        let num = |v: Option<&Value>| v.map(|x| x.to_string()).unwrap_or_default();
        let protocol = if d.get("grpc").is_some_and(Value::is_object) {
            Protocol::Grpc
        } else if d.get("ws").is_some_and(Value::is_object) {
            Protocol::Ws
        } else if d.get("web").is_some_and(Value::is_object) {
            Protocol::Web
        } else if d.get("android").is_some_and(Value::is_object) {
            Protocol::Android
        } else if d.get("desktop").is_some_and(Value::is_object) {
            Protocol::Desktop
        } else {
            Protocol::Http
        };
        let key = format!("/{}", protocol.key());
        let (streaming, message) = match (d.pointer("/grpc/messages"), d.pointer("/grpc/message")) {
            (Some(m), _) => (true, serde_json::to_string_pretty(m).unwrap_or_default()),
            (None, Some(m)) => (false, serde_json::to_string_pretty(m).unwrap_or_default()),
            _ => (false, String::new()),
        };
        let grpc = GrpcForm {
            target: text(d.pointer("/grpc/target")),
            method: text(d.pointer("/grpc/method")),
            message,
            streaming,
            metadata: object(d, "/grpc/metadata"),
        };
        let ws = WsForm {
            url: text(d.pointer("/ws/url")),
            headers: object(d, "/ws/headers"),
            query: object(d, "/ws/query"),
            subprotocols: d.pointer("/ws/subprotocols").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default(),
            steps: d.pointer("/ws/steps").and_then(Value::as_array).map(|a| a.iter().map(StepForm::from_value).collect()).unwrap_or_default(),
        };
        let kind = ui_kind(protocol);
        let ui_key = format!("/{}", kind.key());
        let ui = UiForm {
            target: text(d.pointer(if kind == UiKind::Web { "/web/url" } else if kind == UiKind::Android { "/android/app" } else { "/desktop/app" })),
            window: text(d.pointer("/desktop/window")),
            restart: d.pointer("/desktop/restart").and_then(Value::as_bool).unwrap_or(false),
            activity: text(d.pointer("/android/activity")),
            apk: text(d.pointer("/android/apk")),
            clear_data: d.pointer("/android/clearData").and_then(Value::as_bool).unwrap_or(false),
            attach: d.pointer("/android/attach").and_then(Value::as_bool).unwrap_or(false),
            steps: d.pointer(&format!("{ui_key}/steps")).and_then(Value::as_array).map(|a| a.iter().map(|s| UiStepForm::from_value(s, kind)).collect()).unwrap_or_default(),
        };
        let (data_mode, cases_inline, cases_file) = match d.get("cases") {
            Some(Value::Array(a)) => (1, serde_json::to_string_pretty(a).unwrap_or_default(), String::new()),
            Some(Value::Object(o)) => (2, String::new(), text(o.get("file"))),
            _ => (0, String::new(), String::new()),
        };
        CaseForm {
            name: text(d.get("name")),
            protocol,
            grpc,
            ws,
            ui,
            method: text(d.pointer("/request/method")).to_uppercase(),
            url: text(d.pointer("/request/url")),
            status,
            legacy_status,
            headers: object(d, "/request/headers"),
            query: object(d, "/request/query"),
            body: BodyForm {
                mode,
                raw_json,
                text: body_text,
                form: object(d, "/request/body/form"),
                multipart: object(d, "/request/body/multipart"),
            },
            expect_headers: object(d, "/expect/headers"),
            expect_body: object(d, "/expect/body"),
            capture: object(d, "/capture"),
            tests: d
                .get("tests")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"))
                .unwrap_or_default(),
            pre: text(d.get("pre")),
            post: text(d.get("post")),
            tags: d
                .get("tags")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))
                .unwrap_or_default(),
            // 界面用例的超时规范写法是 timeout（人话，如 "15s"），旧写法 timeoutMs 也认；接口 / gRPC / WebSocket 仍是 timeoutMs 毫秒数
            timeout: match d.pointer(&format!("{key}/timeout")) {
                Some(Value::String(t)) => t.clone(),
                Some(x) => x.to_string(),
                None => num(d.pointer(&format!("{key}/timeoutMs"))),
            },
            retry: num(d.pointer("/request/retry")),
            skip: d.get("skip").and_then(Value::as_bool).unwrap_or(false),
            insecure: d.pointer(&format!("{key}/insecure")).and_then(Value::as_bool).unwrap_or(false),
            follow_redirects: d.pointer("/request/followRedirects").and_then(Value::as_bool).unwrap_or(false),
            data_mode,
            cases_inline,
            cases_file,
        }
    }

    /// 表单 → 文档：以 base（打开时的文件）为底，只改与打开时不同的字段
    pub fn to_doc(&self, base: &Value) -> Result<Value, String> {
        let was = CaseForm::from_doc(base);
        let mut d = if base.is_object() { base.clone() } else { json!({}) };
        let obj = d.as_object_mut().expect("object");

        if self.name != was.name {
            let n = self.name.trim();
            set(obj, "name", (!n.is_empty()).then(|| Value::String(n.into())));
        }
        if self.tags != was.tags {
            let tags: Vec<Value> = self.tags.split(',').map(str::trim).filter(|t| !t.is_empty()).map(|t| Value::String(t.into())).collect();
            set(obj, "tags", (!tags.is_empty()).then_some(Value::Array(tags)));
        }
        if self.skip != was.skip {
            set(obj, "skip", self.skip.then_some(Value::Bool(true)));
        }
        if (self.data_mode, &self.cases_inline, &self.cases_file) != (was.data_mode, &was.cases_inline, &was.cases_file) {
            let cases = match self.data_mode {
                1 if !self.cases_inline.trim().is_empty() => {
                    let v: Value = serde_json::from_str(&self.cases_inline).map_err(|e| format!("数据驱动（内联）JSON 解析失败: {e}"))?;
                    if !v.is_array() {
                        return Err("数据驱动（内联）必须是对象数组".into());
                    }
                    Some(v)
                }
                2 => Some(self.cases_file.trim()).filter(|f| !f.is_empty()).map(|f| json!({ "file": f })),
                _ => None,
            };
            set(obj, "cases", cases);
        }

        // expect：状态码 / 响应头 / 响应体各自独立比较
        let mut expect = obj.get("expect").and_then(Value::as_object).cloned().unwrap_or_default();
        if (&self.status, self.legacy_status) != (&was.status, was.legacy_status) || self.protocol != was.protocol {
            let st = status_value(&self.status, self.protocol)?;
            if self.legacy_status {
                set(obj, "expectStatus", st);
            } else {
                obj.remove("expectStatus");
                set(&mut expect, "status", st);
            }
        }
        if self.expect_headers != was.expect_headers {
            set(&mut expect, "headers", (!self.expect_headers.is_empty()).then(|| Value::Object(self.expect_headers.clone())));
        }
        if self.expect_body != was.expect_body {
            set(&mut expect, "body", (!self.expect_body.is_empty()).then(|| Value::Object(self.expect_body.clone())));
        }
        if obj.get("expect").and_then(Value::as_object) != Some(&expect) {
            set(obj, "expect", (!expect.is_empty()).then_some(Value::Object(expect)));
        }
        if self.capture != was.capture {
            set(obj, "capture", (!self.capture.is_empty()).then(|| Value::Object(self.capture.clone())));
        }

        if self.tests != was.tests {
            let tests: Vec<Value> = self.tests.lines().map(str::trim).filter(|l| !l.is_empty()).map(|l| Value::String(l.into())).collect();
            set(obj, "tests", (!tests.is_empty()).then_some(Value::Array(tests)));
        }
        for (key, now, then) in [("pre", &self.pre, &was.pre), ("post", &self.post, &was.post)] {
            if now != then {
                set(obj, key, (!now.trim().is_empty()).then(|| Value::String(now.trim_end().into())));
            }
        }

        // 协议切换：换掉请求部分（新协议的字段全部写入），断言 / 捕获 / 脚本保留
        let switched = self.protocol != was.protocol;
        if switched {
            for p in Protocol::ALL {
                if p != self.protocol {
                    obj.remove(p.key());
                }
            }
        }
        let blank = CaseForm::default();
        let was = if switched { &blank } else { &was };
        let slot = obj.entry(self.protocol.key()).or_insert_with(|| json!({}));
        if !slot.is_object() {
            *slot = json!({});
        }
        let req = slot.as_object_mut().expect("object");
        match self.protocol {
            Protocol::Http => self.write_http(req, was)?,
            Protocol::Grpc => self.write_grpc(req, was)?,
            Protocol::Ws => self.write_ws(req, was)?,
            Protocol::Web | Protocol::Android | Protocol::Desktop => self.write_ui(req, was)?,
        }
        let number = |t: &str| -> Result<Option<Value>, String> {
            let t = t.trim();
            if t.is_empty() {
                return Ok(None);
            }
            t.parse::<u64>().map(|n| Some(json!(n))).map_err(|_| format!("「{t}」不是非负整数"))
        };
        if self.timeout != was.timeout {
            if matches!(self.protocol, Protocol::Web | Protocol::Android | Protocol::Desktop) {
                // 界面用例：写人话的 timeout（输入数字当毫秒，换成 15s 这样的写法），并去掉旧的 timeoutMs
                let t = self.timeout.trim();
                let text = if t.is_empty() {
                    None
                } else if let Ok(ms) = t.parse::<u64>() {
                    Some(codebaby_core::config::ui::format_duration_ms(ms))
                } else {
                    codebaby_core::config::ui::parse_duration_ms(t).map_err(|e| format!("超时：{e}"))?;
                    Some(t.to_string())
                };
                set(req, "timeoutMs", None);
                set(req, "timeout", text.map(Value::String));
            } else {
                set(req, "timeoutMs", number(&self.timeout)?);
            }
        }
        if self.insecure != was.insecure {
            set(req, "insecure", self.insecure.then_some(Value::Bool(true)));
        }
        if self.protocol == Protocol::Http {
            if self.retry != was.retry {
                set(req, "retry", number(&self.retry)?);
            }
            if self.follow_redirects != was.follow_redirects {
                set(req, "followRedirects", self.follow_redirects.then_some(Value::Bool(true)));
            }
        }
        Ok(d)
    }
}

impl CaseForm {
    fn write_http(&self, req: &mut Map<String, Value>, was: &CaseForm) -> Result<(), String> {
        if self.method != was.method || !req.contains_key("method") {
            let m = if self.method.is_empty() { "GET".to_string() } else { self.method.clone() };
            req.insert("method".into(), Value::String(m));
        }
        if self.url != was.url || !req.contains_key("url") {
            let url = if self.url.trim().is_empty() && !req.contains_key("url") { "{{baseUrl}}/".to_string() } else { self.url.trim().to_string() };
            req.insert("url".into(), Value::String(url));
        }
        if self.headers != was.headers {
            set(req, "headers", (!self.headers.is_empty()).then(|| Value::Object(self.headers.clone())));
        }
        if self.query != was.query {
            set(req, "query", (!self.query.is_empty()).then(|| Value::Object(self.query.clone())));
        }
        if self.body != was.body {
            let b = &self.body;
            let v = match b.mode {
                // raw JSON：合法 JSON 存 {"json"}；不合法则按文本原样发送（界面上有提示）
                1 if !b.text.trim().is_empty() && b.raw_json => Some(match serde_json::from_str::<Value>(&b.text) {
                    Ok(v) => json!({ "json": v }),
                    Err(_) => json!({ "text": b.text }),
                }),
                1 if !b.text.trim().is_empty() => Some(json!({ "text": b.text })),
                2 => Some(json!({ "form": b.form })),
                3 => Some(json!({ "multipart": b.multipart })),
                _ => None,
            };
            set(req, "body", v);
        }
        Ok(())
    }

    fn write_grpc(&self, req: &mut Map<String, Value>, was: &CaseForm) -> Result<(), String> {
        let (g, w) = (&self.grpc, &was.grpc);
        if g.target != w.target || !req.contains_key("target") {
            let t = if g.target.trim().is_empty() { "{{grpcTarget}}" } else { g.target.trim() };
            req.insert("target".into(), Value::String(t.into()));
        }
        if g.method != w.method || !req.contains_key("method") {
            req.insert("method".into(), Value::String(g.method.trim().into()));
        }
        if (&g.message, g.streaming) != (&w.message, w.streaming) {
            req.remove("message");
            req.remove("messages");
            let t = g.message.trim();
            if !t.is_empty() {
                let v: Value = serde_json::from_str(t).map_err(|e| format!("请求消息不是合法 JSON：{e}"))?;
                match (g.streaming, v) {
                    (true, Value::Array(items)) => req.insert("messages".into(), Value::Array(items)),
                    (true, one) => req.insert("messages".into(), Value::Array(vec![one])),
                    (false, Value::Array(_)) => return Err("单条消息要写成对象；要发多条请打开「多条消息（流式）」".into()),
                    (false, one) => req.insert("message".into(), one),
                };
            }
        }
        if g.metadata != w.metadata {
            set(req, "metadata", (!g.metadata.is_empty()).then(|| Value::Object(g.metadata.clone())));
        }
        Ok(())
    }

    fn write_ws(&self, req: &mut Map<String, Value>, was: &CaseForm) -> Result<(), String> {
        let (s, w) = (&self.ws, &was.ws);
        if s.url != w.url || !req.contains_key("url") {
            let u = if s.url.trim().is_empty() { "{{wsBase}}/" } else { s.url.trim() };
            req.insert("url".into(), Value::String(u.into()));
        }
        if s.headers != w.headers {
            set(req, "headers", (!s.headers.is_empty()).then(|| Value::Object(s.headers.clone())));
        }
        if s.query != w.query {
            set(req, "query", (!s.query.is_empty()).then(|| Value::Object(s.query.clone())));
        }
        if s.subprotocols != w.subprotocols {
            let p: Vec<Value> = s.subprotocols.split([',', '，']).map(str::trim).filter(|x| !x.is_empty()).map(|x| Value::String(x.into())).collect();
            set(req, "subprotocols", (!p.is_empty()).then_some(Value::Array(p)));
        }
        if s.steps != w.steps || (!req.contains_key("steps") && !s.steps.is_empty()) {
            let steps: Vec<Value> = s.steps.iter().enumerate().map(|(i, st)| st.to_value(i + 1)).collect::<Result<_, _>>()?;
            set(req, "steps", (!steps.is_empty() || req.contains_key("steps")).then_some(Value::Array(steps)));
        }
        Ok(())
    }
}

impl CaseForm {
    fn write_ui(&self, req: &mut Map<String, Value>, was: &CaseForm) -> Result<(), String> {
        let kind = ui_kind(self.protocol);
        let (s, w) = (&self.ui, &was.ui);
        let key = if kind == UiKind::Web { "url" } else { "app" };
        if s.target != w.target || !req.contains_key(key) {
            let t = s.target.trim();
            let t = if t.is_empty() && kind == UiKind::Web { "{{webBase}}/" } else { t };
            req.insert(key.into(), Value::String(t.into()));
        }
        if kind == UiKind::Desktop {
            if s.window != w.window {
                set(req, "window", (!s.window.trim().is_empty()).then(|| Value::String(s.window.trim().into())));
            }
            if s.restart != w.restart {
                set(req, "restart", s.restart.then_some(Value::Bool(true)));
            }
        }
        if kind == UiKind::Android {
            for (k, now, then) in [("activity", &s.activity, &w.activity), ("apk", &s.apk, &w.apk)] {
                if now != then {
                    set(req, k, (!now.trim().is_empty()).then(|| Value::String(now.trim().into())));
                }
            }
            if s.clear_data != w.clear_data {
                set(req, "clearData", s.clear_data.then_some(Value::Bool(true)));
            }
            if s.attach != w.attach {
                set(req, "attach", s.attach.then_some(Value::Bool(true)));
            }
        }
        // 从别的协议切过来：步骤按新协议重写（Android 的点击写 tap）
        let fresh = was.protocol != self.protocol;
        if s.steps != w.steps || !req.contains_key("steps") || fresh {
            let steps: Vec<Value> = s
                .steps
                .iter()
                .enumerate()
                .map(|(i, st)| UiStepForm { raw: if fresh { None } else { st.raw.clone() }, ..st.clone() }.to_value(i + 1, kind))
                .collect::<Result<_, _>>()?;
            req.insert("steps".into(), Value::Array(steps));
        }
        Ok(())
    }
}

/// 状态文本 → 文件值：HTTP / WebSocket 写 `200` / `200, 201`；gRPC 写 `OK` / `OK, NOT_FOUND`（或 0–16）；都可写 `{{status}}`。
/// 写错时报错而不是悄悄丢掉
pub fn status_value(t: &str, protocol: Protocol) -> Result<Option<Value>, String> {
    let t = t.trim();
    if t.is_empty() {
        return Ok(None);
    }
    if t.starts_with("{{") && t.ends_with("}}") {
        return Ok(Some(Value::String(t.into())));
    }
    let parts: Vec<&str> = t.split([',', ' ', '，']).map(str::trim).filter(|x| !x.is_empty()).collect();
    if matches!(protocol, Protocol::Android | Protocol::Desktop) {
        return Err(format!("{} 用例没有状态码：清空它，用步骤里的断言验证界面", protocol.label()));
    }
    if protocol == Protocol::Grpc {
        let names: Vec<String> = parts
            .iter()
            .map(|p| codebaby_core::config::protocols::parse_grpc_code(p).map(codebaby_core::config::protocols::grpc_code_name))
            .collect::<Option<_>>()
            .ok_or_else(|| format!("gRPC 状态「{t}」写法不对：写 OK、NOT_FOUND 或 OK, NOT_FOUND"))?;
        // 原文写的是数字就保留数字
        if parts.iter().all(|p| p.parse::<u16>().is_ok()) {
            let codes: Vec<u16> = parts.iter().map(|p| p.parse().unwrap()).collect();
            return Ok(Some(if codes.len() == 1 { json!(codes[0]) } else { json!(codes) }));
        }
        return Ok(Some(if names.len() == 1 { json!(names[0]) } else { json!(names) }));
    }
    let codes: Vec<u16> = parts
        .iter()
        .map(|x| x.parse::<u16>().ok().filter(|c| (100..600).contains(c)))
        .collect::<Option<_>>()
        .ok_or_else(|| format!("状态码「{t}」写法不对：写 200、200, 201 或 {{{{status}}}}"))?;
    Ok(Some(if codes.len() == 1 { json!(codes[0]) } else { json!(codes) }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// 代码块里的 `json case` 用例
    fn doc_cases(path: &Path) -> Vec<Value> {
        let text = std::fs::read_to_string(path).unwrap();
        let mut out = vec![];
        let mut cur: Option<String> = None;
        for line in text.lines() {
            match (&mut cur, line.trim_start().strip_prefix("```")) {
                (None, Some("json case")) => cur = Some(String::new()),
                (Some(b), Some(_)) => {
                    out.push(serde_json::from_str(b).unwrap());
                    cur = None;
                }
                (Some(b), None) => {
                    b.push_str(line);
                    b.push('\n');
                }
                _ => {}
            }
        }
        out
    }

    fn corpus() -> Vec<Value> {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut docs: Vec<Value> = std::fs::read_dir(repo.join("docs/guide"))
            .unwrap()
            .flatten()
            .flat_map(|e| doc_cases(&e.path()))
            .collect();
        docs.extend(doc_cases(&repo.join("README.md")));
        // 表单难以表达、最容易被「打开即改写」的写法
        docs.extend([
            json!({"name": "  带空格的名字 ", "request": {"method": "GET", "url": " {{baseUrl}}/x "}}),
            json!({"request": {"method": "GET", "url": "{{baseUrl}}/x"}, "expect": {"status": [200]}}),
            json!({"request": {"method": "GET", "url": "{{baseUrl}}/x"}, "expectStatus": [200, 409], "tests": ["data.list\n  .every(x => x.ok)", "  res.status === 200  "]}),
            json!({"request": {"method": "POST", "url": "{{baseUrl}}/x", "body": {"text": "a=1\n"}}, "pre": "req.headers.a = 1\n", "post": "vars.x = 1\n\n"}),
            json!({"request": {"method": "POST", "url": "{{baseUrl}}/x", "body": {"json": {"b": 1, "a": [1.50, {"z": null}]}}}}),
            json!({"request": {"method": "POST", "url": "{{baseUrl}}/x", "body": {"form": {"a": "1"}}, "headers": {"X": {"value": "1", "disabled": true}}}}),
            json!({"request": {"method": "POST", "url": "{{baseUrl}}/x", "body": {"multipart": {"f": {"file": "a.png", "type": "image/png"}}}, "timeoutMs": 500, "retry": 0, "insecure": true}}),
            json!({"skip": true, "tags": ["a", "b"], "cases": {"file": "data.csv"}, "request": {"method": "DELETE", "url": "{{baseUrl}}/x"}, "expect": {"status": "{{status}}"}}),
            json!({"request": {"method": "GET", "url": "{{baseUrl}}/x"}, "expect": {"body": {"a": {"eq": 1}, "b": {"gt": 0, "lt": 9}}}, "capture": {"t": "header:X-T"}, "unknownField": 1}),
            json!({"request": {"method": "GET", "url": "{{baseUrl}}/x"}, "tags": [], "expect": {}}),
        ]);
        assert!(docs.len() >= 25, "{}", docs.len());
        docs
    }

    /// 打开后不做任何修改就保存：文件一个字节都不变
    #[test]
    fn untouched_form_writes_back_the_same_document() {
        for d in corpus() {
            let back = CaseForm::from_doc(&d).to_doc(&d).unwrap();
            assert_eq!(back, d);
        }
    }

    /// 界面用例的用例级超时：规范写法 timeout（人话），旧写法 timeoutMs 也认；改动后写 timeout 并去掉旧的；接口用例不受影响
    #[test]
    fn ui_case_timeout_reads_and_writes_human_text() {
        let d = json!({"name": "x", "android": {"app": "com.a.b", "timeout": "15s", "steps": [{"tap": {"text": "a"}}]}});
        let f = CaseForm::from_doc(&d);
        assert_eq!(f.timeout, "15s");
        assert_eq!(f.to_doc(&d).unwrap(), d, "没改原样写回");
        let mut g = f.clone();
        g.timeout = "2m".into();
        assert_eq!(g.to_doc(&d).unwrap()["android"]["timeout"], "2m");
        g.timeout = "20000".into();
        assert_eq!(g.to_doc(&d).unwrap()["android"]["timeout"], "20s", "输入数字当毫秒，写成人话");
        g.timeout = String::new();
        assert!(g.to_doc(&d).unwrap()["android"].get("timeout").is_none());
        g.timeout = "很久".into();
        assert!(g.to_doc(&d).is_err());
        // 旧写法 timeoutMs 读得出来，改动后换成 timeout
        let old = json!({"name": "x", "web": {"url": "u", "timeoutMs": 5000, "steps": []}});
        let f = CaseForm::from_doc(&old);
        assert_eq!(f.timeout, "5000");
        let mut g = f.clone();
        g.timeout = "8s".into();
        let w = g.to_doc(&old).unwrap();
        assert_eq!((w["web"].get("timeoutMs"), &w["web"]["timeout"]), (None, &json!("8s")));
        // 接口用例仍是 timeoutMs 毫秒数
        let api = json!({"name": "x", "request": {"method": "GET", "url": "{{baseUrl}}/a", "timeoutMs": 800}});
        let mut g = CaseForm::from_doc(&api);
        g.timeout = "900".into();
        assert_eq!(g.to_doc(&api).unwrap()["request"]["timeoutMs"], 900);
    }

    /// 改了什么就只改什么，结果仍是合法用例
    #[test]
    fn edits_touch_only_their_field() {
        let base = json!({
            "name": "x",
            "request": {"method": "GET", "url": "{{baseUrl}}/a", "body": {"json": {"k": 1}}},
            "expectStatus": [200, 409],
            "tests": ["data.list\n  .every(x => x.ok)"],
            "post": "vars.x = 1\n"
        });
        let f = CaseForm::from_doc(&base);
        // 旧写法状态码：原位改写
        let mut g = f.clone();
        g.status = "201".into();
        let d = g.to_doc(&base).unwrap();
        assert_eq!((d["expectStatus"].clone(), d["tests"].clone(), d["post"].clone()), (json!(201), base["tests"].clone(), base["post"].clone()));
        // 新增响应体断言：expect 出现，其余不动
        let mut g = f.clone();
        g.expect_body.insert("code".into(), json!(0));
        let d = g.to_doc(&base).unwrap();
        assert_eq!(d["expect"], json!({"body": {"code": 0}}));
        assert_eq!(d["request"], base["request"]);
        // 改写 tests：按行拆分
        let mut g = f.clone();
        g.tests = "a === 1\n\n  b === 2 ".into();
        assert_eq!(g.to_doc(&base).unwrap()["tests"], json!(["a === 1", "b === 2"]));
        // 请求体改成非法 JSON：按文本发送；清空：去掉 body
        let mut g = f.clone();
        g.body.text = "{oops".into();
        assert_eq!(g.to_doc(&base).unwrap()["request"]["body"], json!({"text": "{oops"}));
        g.body.text = " ".into();
        assert!(g.to_doc(&base).unwrap()["request"].get("body").is_none());
        // 写错的数字 / 状态码：报错，不悄悄丢掉
        let mut g = f.clone();
        g.timeout = "abc".into();
        assert!(g.to_doc(&base).unwrap_err().contains("abc"));
        let mut g = f.clone();
        g.status = "2OO".into();
        assert!(g.to_doc(&base).unwrap_err().contains("状态码"));
        // 新写法：清空状态码且没有其他断言 → 去掉 expect
        let base2 = json!({"request": {"method": "GET", "url": "{{baseUrl}}/a"}, "expect": {"status": 200}});
        let mut g = CaseForm::from_doc(&base2);
        g.status = "".into();
        assert!(g.to_doc(&base2).unwrap().get("expect").is_none());
        g.status = "200, 201".into();
        assert_eq!(g.to_doc(&base2).unwrap()["expect"], json!({"status": [200, 201]}));
        for doc in [g.to_doc(&base2).unwrap(), f.to_doc(&base).unwrap()] {
            serde_json::from_value::<codebaby_core::config::TestCase>(doc).unwrap();
        }
    }

    /// 切换协议：请求部分整体换掉，断言 / 捕获 / 标签保留；gRPC 状态写名字
    #[test]
    fn switching_protocol_replaces_only_the_request_part() {
        let base = json!({
            "name": "x",
            "tags": ["smoke"],
            "request": {"method": "GET", "url": "{{baseUrl}}/a", "timeoutMs": 800},
            "expect": {"status": 200, "body": {"id": 1}},
            "capture": {"id": "id"}
        });
        let mut g = CaseForm::from_doc(&base);
        g.protocol = Protocol::Grpc;
        g.grpc.target = "{{grpcTarget}}".into();
        g.grpc.method = "shop.v1.OrderService/GetOrder".into();
        g.grpc.message = r#"{"id": "{{id:number}}"}"#.into();
        g.grpc.metadata.insert("authorization".into(), json!("Bearer {{token}}"));
        g.status = "OK, NOT_FOUND".into();
        let d = g.to_doc(&base).unwrap();
        assert!(d.get("request").is_none());
        assert_eq!(
            d["grpc"],
            json!({"target": "{{grpcTarget}}", "method": "shop.v1.OrderService/GetOrder", "message": {"id": "{{id:number}}"}, "metadata": {"authorization": "Bearer {{token}}"}, "timeoutMs": 800})
        );
        assert_eq!(d["expect"], json!({"status": ["OK", "NOT_FOUND"], "body": {"id": 1}}));
        assert_eq!((d["capture"].clone(), d["tags"].clone()), (base["capture"].clone(), base["tags"].clone()));
        // 多条消息：写 messages
        g.grpc.streaming = true;
        g.grpc.message = r#"[{"sku": "A"}, {"sku": "B"}]"#.into();
        let d = g.to_doc(&base).unwrap();
        assert_eq!((d["grpc"].get("message"), &d["grpc"]["messages"]), (None, &json!([{"sku": "A"}, {"sku": "B"}])));
        // 写错的状态名：报错
        g.status = "NOTFOUND".into();
        assert!(g.to_doc(&base).is_err());
        // 再切到 WebSocket：grpc 消失
        let mut w = CaseForm::from_doc(&base);
        w.protocol = Protocol::Ws;
        w.ws.url = "{{wsBase}}/chat".into();
        w.status = "101".into();
        let d = w.to_doc(&base).unwrap();
        assert!(d.get("request").is_none() && d.get("grpc").is_none());
        assert_eq!(d["ws"]["url"], json!("{{wsBase}}/chat"));
        for doc in [d, g.to_doc(&base).unwrap_or_default()] {
            if !doc.is_null() {
                serde_json::from_value::<codebaby_core::config::TestCase>(doc).unwrap();
            }
        }
    }

    /// 界面步骤：没改的原样写回；改过的按动作写（Android 点击写 tap）；文字直接当可见文字定位
    #[test]
    fn ui_steps_round_trip_and_edit() {
        let base = json!({
            "web": {"url": "{{webBase}}/login", "steps": [
                {"fill": {"label": "手机号"}, "value": "138"},
                {"click": {"role": "button", "name": "登录"}, "timeoutMs": 5000},
                {"waitFor": {"url": "/home"}, "capture": {"u": "url"}}
            ]},
            "expect": {"status": 200}
        });
        let f = CaseForm::from_doc(&base);
        assert_eq!(f.protocol, Protocol::Web);
        assert_eq!(f.ui.steps.iter().map(|s| s.action).collect::<Vec<_>>(), [UiAction::Fill, UiAction::Click, UiAction::WaitFor]);
        assert_eq!(f.ui.steps[1].target, r#"{"role":"button","name":"登录"}"#);
        assert_eq!(f.to_doc(&base).unwrap(), base);
        let mut g = f.clone();
        g.ui.steps[1].target = "提交".into();
        let mut add = UiStepForm::new(UiAction::Press);
        add.target = "Enter".into();
        g.ui.steps.push(add);
        g.ui.steps.push(UiStepForm::new(UiAction::Back));
        let d = g.to_doc(&base).unwrap();
        assert_eq!(d["web"]["steps"][1], json!({"click": {"text": "提交"}, "timeoutMs": 5000}));
        assert_eq!((&d["web"]["steps"][3], &d["web"]["steps"][4]), (&json!({"press": "Enter"}), &json!({"back": true})));
        serde_json::from_value::<codebaby_core::config::TestCase>(d).unwrap();
        let mut bad = f.clone();
        bad.ui.steps[0].target = "{oops".into();
        assert!(bad.to_doc(&base).unwrap_err().contains("第 1 步"));

        // 切到 Android：web 消失，点击写 tap，清除数据写在 android 上
        let mut a = CaseForm::from_doc(&base);
        a.protocol = Protocol::Android;
        a.ui.target = "com.example.shop".into();
        a.ui.clear_data = true;
        a.status = String::new();
        let d = a.to_doc(&base).unwrap();
        assert!(d.get("web").is_none());
        assert_eq!(d["android"]["app"], "com.example.shop");
        assert_eq!(d["android"]["clearData"], true);
        assert_eq!(d["android"]["steps"][1], json!({"tap": {"role": "button", "name": "登录"}, "timeoutMs": 5000}));
        serde_json::from_value::<codebaby_core::config::TestCase>(d).unwrap();

        // 切到 Mac 桌面：点击仍写 click，窗口与重启写在 desktop 上
        let mut m = CaseForm::from_doc(&base);
        m.protocol = Protocol::Desktop;
        m.ui.target = "TextEdit".into();
        m.ui.window = "Untitled".into();
        m.ui.restart = true;
        m.status = String::new();
        let d = m.to_doc(&base).unwrap();
        assert!(d.get("web").is_none());
        assert_eq!((&d["desktop"]["app"], &d["desktop"]["window"], &d["desktop"]["restart"]), (&json!("TextEdit"), &json!("Untitled"), &json!(true)));
        assert_eq!(d["desktop"]["steps"][1], json!({"click": {"role": "button", "name": "登录"}, "timeoutMs": 5000}));
        serde_json::from_value::<codebaby_core::config::TestCase>(d.clone()).unwrap();
        let back = CaseForm::from_doc(&d);
        assert_eq!((back.protocol, back.ui.target.as_str(), back.ui.window.as_str(), back.ui.restart), (Protocol::Desktop, "TextEdit", "Untitled", true));
    }

    /// 人话时长：`"3s-8s"` / `"2.5s"` / `"timeout": "15s"` 读出来是原文，改动后仍写成原文，不被换成毫秒数
    #[test]
    fn ui_human_durations_round_trip_and_report_mistakes() {
        use codebaby_core::config::UiKind;
        let steps = json!([
            {"wait": "3s-8s"},
            {"wait": "2.5s"},
            {"tap": {"ocr": "11-28"}, "timeout": "15s"},
            {"repeat": {"for": "forever", "cycle": {"date": ["11-28", "11-29"]}, "steps": [{"wait": "1s"}]}}
        ]);
        let forms: Vec<UiStepForm> = steps.as_array().unwrap().iter().map(|v| UiStepForm::from_value(v, UiKind::Android)).collect();
        assert_eq!((forms[0].target.as_str(), forms[1].target.as_str(), forms[2].timeout.as_str()), ("3s-8s", "2.5s", "15s"));
        assert!(forms[3].target.contains("\"cycle\""), "{}", forms[3].target);
        for (f, v) in forms.iter().zip(steps.as_array().unwrap()) {
            assert_eq!(&f.to_value(1, UiKind::Android).unwrap(), v, "没改原样写回");
        }
        // 改成别的人话时长：按文字写回；毫秒数仍写 timeoutMs
        let mut w = forms[0].clone();
        w.target = "1s~2s".into();
        assert_eq!(w.to_value(1, UiKind::Android).unwrap(), json!({"wait": "1s~2s"}));
        let mut t = forms[2].clone();
        t.timeout = "2m".into();
        assert_eq!(t.to_value(1, UiKind::Android).unwrap(), json!({"tap": {"ocr": "11-28"}, "timeout": "2m"}));
        t.timeout = "5000".into();
        assert_eq!(t.to_value(1, UiKind::Android).unwrap(), json!({"tap": {"ocr": "11-28"}, "timeoutMs": 5000}));
        // 写错报第几步
        w.target = "三秒".into();
        assert!(w.to_value(4, UiKind::Android).unwrap_err().contains("第 4 步"));
        t.timeout = "很久".into();
        assert!(t.to_value(2, UiKind::Android).unwrap_err().contains("第 2 步"));
    }

    /// 循环 / 条件 / 随机等待 / 通知：读出来是文本、没改原样写回，改了按动作写；写错报第几步
    #[test]
    fn ui_control_steps_round_trip_and_report_mistakes() {
        use codebaby_core::config::UiKind;
        let steps = json!([
            {"repeat": {"times": 5, "until": {"ocr": "有号"}, "steps": [{"tap": {"ocr": "11-28"}}, {"wait": {"min": 3000, "max": 8000}}]}},
            {"if": {"when": {"ocr": "有号"}, "then": [{"notify": "有号了"}]}},
            {"notify": {"message": "{{date}} 有号", "webhook": "https://hooks.example.com/x"}, "optional": true},
            {"wait": {"min": 1, "max": 2}},
            {"wait": 500}
        ]);
        let forms: Vec<UiStepForm> = steps.as_array().unwrap().iter().map(|v| UiStepForm::from_value(v, UiKind::Android)).collect();
        assert_eq!(forms.iter().map(|f| f.action).collect::<Vec<_>>(), [UiAction::Repeat, UiAction::If, UiAction::Notify, UiAction::Wait, UiAction::Wait]);
        // 文本：repeat / if 是整个对象，notify 带 webhook 时是对象、只有文字时是文字本身，wait 是毫秒数或区间对象
        assert!(forms[0].target.starts_with('{') && forms[0].target.contains("\"times\":5"), "{}", forms[0].target);
        assert!(forms[2].target.starts_with('{') && forms[2].optional);
        let range: Value = serde_json::from_str(&forms[3].target).unwrap();
        assert_eq!(range, json!({"min": 1, "max": 2}));
        assert_eq!(forms[4].target, "500");
        for (f, v) in forms.iter().zip(steps.as_array().unwrap()) {
            assert_eq!(&f.to_value(1, UiKind::Android).unwrap(), v, "没改应当原样写回");
        }
        // 改一下（让 raw 不再相等）：按动作重新生成
        let mut f = forms[0].clone();
        f.target = r#"{"times": 3, "steps": [{"wait": 1000}]}"#.into();
        assert_eq!(f.to_value(1, UiKind::Android).unwrap(), json!({"repeat": {"times": 3, "steps": [{"wait": 1000}]}}));
        let mut e = UiStepForm::new(UiAction::Notify);
        e.target = "有号了".into();
        assert_eq!(e.to_value(1, UiKind::Web).unwrap(), json!({"notify": "有号了"}));
        e.target = r#"{"message": "x", "webhook": "https://h.example/x"}"#.into();
        e.optional = true;
        assert_eq!(e.to_value(1, UiKind::Web).unwrap(), json!({"notify": {"message": "x", "webhook": "https://h.example/x"}, "optional": true}));
        let mut w = UiStepForm::new(UiAction::Wait);
        w.target = r#"{"min": 3000, "max": 8000}"#.into();
        assert_eq!(w.to_value(1, UiKind::Web).unwrap(), json!({"wait": {"min": 3000, "max": 8000}}));
        // 新建时的默认内容本身是合法写法
        for a in [UiAction::Repeat, UiAction::If, UiAction::Wait] {
            let v = UiStepForm::new(a).to_value(1, UiKind::Web).unwrap();
            serde_json::from_value::<codebaby_core::config::UiStep>(v).unwrap();
        }
        // 写错：报第几步和原因
        let bad = |a: UiAction, t: &str| {
            let mut f = UiStepForm::new(a);
            f.target = t.into();
            f.to_value(3, UiKind::Web).unwrap_err()
        };
        assert!(bad(UiAction::Repeat, "{oops").contains("第 3 步"));
        assert!(bad(UiAction::Repeat, r#"{"times": 2, "step": []}"#).contains("写法不对"));
        assert!(bad(UiAction::If, r#"{"then": []}"#).contains("写法不对"));
        assert!(bad(UiAction::Notify, "  ").contains("没有写内容"));
        assert!(bad(UiAction::Notify, r#"{"cmd": "x"}"#).contains("写法不对"));
        assert!(bad(UiAction::Wait, r#"{"min": 9, "max": 1}"#).contains("不能大于"));
        assert!(bad(UiAction::Wait, "abc").contains("看不懂时长"));
    }

    /// 对话步骤：没改的步骤原样写回，改过的按动作写；内容写错报第几步
    #[test]
    fn ws_steps_round_trip_and_edit() {
        let base = json!({
            "ws": {
                "url": "{{wsBase}}/chat",
                "steps": [
                    {"receive": {"type": "welcome"}, "capture": {"sid": "sid"}},
                    {"send": {"type": "join", "room": 1}},
                    {"wait": 300},
                    {"close": 4000}
                ]
            },
            "expect": {"status": 101}
        });
        let f = CaseForm::from_doc(&base);
        assert_eq!(f.ws.steps.iter().map(|s| s.action).collect::<Vec<_>>(), [WsAction::Receive, WsAction::Send, WsAction::Wait, WsAction::Close]);
        assert_eq!((f.ws.steps[0].capture.as_str(), f.ws.steps[3].content.as_str()), ("sid=sid", "4000"));
        let mut g = f.clone();
        g.ws.steps[1].content = "ping".into();
        g.ws.steps[2].action = WsAction::WaitFor;
        g.ws.steps[2].content = r#"{"$": "pong"}"#.into();
        g.ws.steps[2].capture = "a=x, b=y.z".into();
        g.ws.steps[2].timeout = "2000".into();
        let mut add = StepForm::new(WsAction::Send);
        add.content = r#"{"type": "say", "text": "{{sid}}"}"#.into();
        g.ws.steps.push(add);
        let d = g.to_doc(&base).unwrap();
        assert_eq!(
            d["ws"]["steps"],
            json!([
                {"receive": {"type": "welcome"}, "capture": {"sid": "sid"}},
                {"send": "ping"},
                {"waitFor": {"$": "pong"}, "capture": {"a": "x", "b": "y.z"}, "timeoutMs": 2000},
                {"close": 4000},
                {"send": {"type": "say", "text": "{{sid}}"}}
            ])
        );
        serde_json::from_value::<codebaby_core::config::TestCase>(d).unwrap();
        let mut bad = f.clone();
        bad.ws.steps[0].content = "{oops".into();
        assert!(bad.to_doc(&base).unwrap_err().contains("第 1 步"));
        let mut bad = f.clone();
        bad.ws.steps[2].content = "soon".into();
        assert!(bad.to_doc(&base).unwrap_err().contains("第 3 步"));
    }
}
