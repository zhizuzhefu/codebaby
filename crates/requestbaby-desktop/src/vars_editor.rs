//! 变量表编辑器：项目 / 环境 / 集合三处共用。
//!
//! 覆盖 `vars` 的全部三种形态：普通值、`{"value","secret":true}`、`{"from":"env","name"}`。
//! 未改动的行原样写回（保留数字 / 布尔等原始 JSON 类型，避免无意义 diff）。
use gpui_kit::component::{
    ActiveTheme as _, IconName, IndexPath, Sizable as _, h_flex,
    button::{Button, ButtonVariants as _},
    input::{Input, InputEvent, InputState},
    select::{Select, SelectEvent, SelectState},
    v_flex,
};
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use serde_json::{Map, Value, json};

const KINDS: [&str; 3] = ["普通", "Secret", "环境变量"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Plain,
    Secret,
    Env,
}

impl Kind {
    fn from_label(s: &str) -> Self {
        match s {
            "Secret" => Kind::Secret,
            "环境变量" => Kind::Env,
            _ => Kind::Plain,
        }
    }

    fn index(self) -> usize {
        match self {
            Kind::Plain => 0,
            Kind::Secret => 1,
            Kind::Env => 2,
        }
    }
}

struct Row {
    id: usize,
    key: Entity<InputState>,
    kind: Entity<SelectState<Vec<SharedString>>>,
    value: Entity<InputState>,
    /// 加载时的原始 (key, kind, text, json)：未改动则原样写回
    original: Option<(String, Kind, String, Value)>,
}

pub enum VarsEditorEvent {
    Changed,
}

pub struct VarsEditor {
    rows: Vec<Row>,
    next_id: usize,
    /// 是否允许 secret / 环境变量形态（集合 headers 之类的纯文本表不需要）
    rich: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<VarsEditorEvent> for VarsEditor {}

fn decode(v: &Value) -> (Kind, String) {
    match v {
        Value::Object(o) if o.get("from").and_then(|f| f.as_str()) == Some("env") => {
            (Kind::Env, o.get("name").and_then(|n| n.as_str()).unwrap_or_default().to_string())
        }
        Value::Object(o) => {
            let val = o.get("value").map(text_of).unwrap_or_default();
            let secret = o.get("secret").and_then(|s| s.as_bool()).unwrap_or(false);
            (if secret { Kind::Secret } else { Kind::Plain }, val)
        }
        other => (Kind::Plain, text_of(other)),
    }
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

impl VarsEditor {
    pub fn new(rich: bool) -> Self {
        Self { rows: vec![], next_id: 0, rich, _subscriptions: vec![] }
    }

    fn push_row(&mut self, key: String, kind: Kind, text: String, original: Option<Value>, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.next_id;
        self.next_id += 1;
        let key_state = cx.new(|cx| InputState::new(window, cx).placeholder("变量名").default_value(key.clone()));
        let value_state = cx.new(|cx| InputState::new(window, cx).default_value(text.clone()));
        let kinds: Vec<SharedString> = KINDS.iter().map(|k| SharedString::from(*k)).collect();
        let kind_state = cx.new(|cx| SelectState::new(kinds, Some(IndexPath::new(kind.index())), window, cx));
        for input in [&key_state, &value_state] {
            self._subscriptions.push(cx.subscribe_in(input, window, |_, _, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    cx.emit(VarsEditorEvent::Changed);
                }
            }));
        }
        self._subscriptions.push(cx.subscribe_in(&kind_state, window, |this, _, _: &SelectEvent<Vec<SharedString>>, window, cx| {
            this.sync_placeholders(window, cx);
            cx.emit(VarsEditorEvent::Changed);
            cx.notify();
        }));
        self.rows.push(Row { id, key: key_state, kind: kind_state, value: value_state, original: original.map(|o| (key, kind, text, o)) });
        self.sync_placeholders(window, cx);
    }

    fn sync_placeholders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for r in &self.rows {
            let kind = self.kind_of(r, cx);
            let ph = match kind {
                Kind::Env => "进程环境变量名（缺省同变量名）",
                Kind::Secret => "敏感值（输出时脱敏为 ****）",
                Kind::Plain => "值",
            };
            r.value.update(cx, |s, cx| s.set_placeholder(ph, window, cx));
        }
    }

    fn kind_of(&self, r: &Row, cx: &gpui_kit::App) -> Kind {
        r.kind.read(cx).selected_value().map(|v| Kind::from_label(v)).unwrap_or(Kind::Plain)
    }

    /// 载入 `vars` 对象
    pub fn load(&mut self, vars: Option<&Map<String, Value>>, window: &mut Window, cx: &mut Context<Self>) {
        self.rows.clear();
        self._subscriptions.clear();
        if let Some(vars) = vars {
            for (k, v) in vars {
                let (kind, text) = decode(v);
                self.push_row(k.clone(), kind, text, Some(v.clone()), window, cx);
            }
        }
        cx.notify();
    }

    pub fn to_map(&self, cx: &gpui_kit::App) -> Map<String, Value> {
        let mut out = Map::new();
        for r in &self.rows {
            let key = r.key.read(cx).value().trim().to_string();
            if key.is_empty() {
                continue;
            }
            let kind = if self.rich { self.kind_of(r, cx) } else { Kind::Plain };
            let text = r.value.read(cx).value().to_string();
            if let Some((ok, okind, otext, orig)) = &r.original {
                if *ok == key && *okind == kind && *otext == text {
                    out.insert(key, orig.clone());
                    continue;
                }
            }
            let v = match kind {
                Kind::Plain => Value::String(text),
                Kind::Secret => json!({ "value": text, "secret": true }),
                Kind::Env if text.trim().is_empty() || text.trim() == key => json!({ "from": "env" }),
                Kind::Env => json!({ "from": "env", "name": text.trim() }),
            };
            out.insert(key, v);
        }
        out
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.push_row(String::new(), Kind::Plain, String::new(), None, window, cx);
        cx.emit(VarsEditorEvent::Changed);
        cx.notify();
    }

    fn remove(&mut self, id: usize, cx: &mut Context<Self>) {
        self.rows.retain(|r| r.id != id);
        cx.emit(VarsEditorEvent::Changed);
        cx.notify();
    }
}

impl Render for VarsEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let rich = self.rich;
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .text_color(muted)
                    .child(div().w(px(200.)).child(if rich { "变量名" } else { "名称" }))
                    .when(rich, |this| this.child(div().w(px(112.)).child("类型")))
                    .child(div().flex_1().child("值")),
            )
            .children(self.rows.iter().map(|r| {
                let id = r.id;
                h_flex()
                    .gap_2()
                    .child(div().w(px(200.)).child(Input::new(&r.key).small()))
                    .when(rich, |this| this.child(div().w(px(112.)).child(Select::new(&r.kind).small())))
                    .child(div().flex_1().child(Input::new(&r.value).small()))
                    .child(
                        Button::new(("remove-var", id))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .tooltip("删除")
                            .on_click(cx.listener(move |this, _, _, cx| this.remove(id, cx))),
                    )
            }))
            .child(
                h_flex().child(
                    Button::new("add-var")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Plus)
                        .label(if rich { "添加变量" } else { "添加" })
                        .on_click(cx.listener(|this, _, window, cx| this.add(window, cx))),
                ),
            )
    }
}
