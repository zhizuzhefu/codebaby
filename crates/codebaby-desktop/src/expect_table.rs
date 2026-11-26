//! 断言表格：用例 `expect.body` / `expect.headers` 的表单视图，一行对应文件里的一条。
//!
//! - 每行「字段路径 · 条件 · 值」；条件「等于」直接写字面量，其余写成 `{"gt": 0}` 这样的运算符对象
//! - 值按 JSON 理解（0 / true / null / [1,2] / "带引号的文本"），其余按文本；`{{变量}}` 原样保留
//! - 末尾常驻一行空白行，输入即新增；未改动的行原样写回（保留表格无法表达的写法）
//! - 表单 ↔ 文件的转换在 core（expect::split_matcher / join_matcher），这里只负责界面
use gpui_kit::component::{
    ActiveTheme as _, IconName, IndexPath, Sizable as _, h_flex,
    button::{Button, ButtonVariants as _},
    input::{Input, InputEvent, InputState},
    select::{Select, SelectEvent, SelectState},
    v_flex,
};
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use codebaby_core::expect::{FORM_OPS, join_matcher, split_matcher};
use serde_json::{Map, Value};

type OpSelect = SelectState<Vec<SharedString>>;

struct Row {
    id: usize,
    key: Entity<InputState>,
    op: Entity<OpSelect>,
    value: Entity<InputState>,
    /// 加载时的 (路径, 条件, 值文本, 原始期望值)：未改动则原样写回
    original: Option<(String, &'static str, String, Value)>,
}

pub enum ExpectTableEvent {
    Changed,
}

pub struct ExpectTable {
    key_label: &'static str,
    key_placeholder: &'static str,
    rows: Vec<Row>,
    next_id: usize,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ExpectTableEvent> for ExpectTable {}

fn op_labels() -> Vec<SharedString> {
    FORM_OPS.iter().map(|(_, label)| SharedString::from(*label)).collect()
}

fn op_index(op: &str) -> usize {
    FORM_OPS.iter().position(|(o, _)| *o == op).unwrap_or(0)
}

/// 各条件下值输入框的提示
fn value_hint(op: &str) -> &'static str {
    match op {
        "=" => "0 / ok / true / {{变量}}",
        "type" => "string / integer / number / boolean / null / array / object",
        "exists" => "true（存在）/ false（不存在）",
        "oneOf" => "[\"a\", \"b\"]",
        "len" => "数量，如 2",
        "matches" => "正则，如 ^\\d+$",
        "json" => "{\"gt\": 0, \"lt\": 100}",
        _ => "值",
    }
}

impl ExpectTable {
    pub fn new(key_label: &'static str, key_placeholder: &'static str) -> Self {
        Self { key_label, key_placeholder, rows: vec![], next_id: 0, _subscriptions: vec![] }
    }

    fn op_of(&self, r: &Row, cx: &gpui_kit::App) -> &'static str {
        let label = r.op.read(cx).selected_value().cloned().unwrap_or_default();
        FORM_OPS.iter().find(|(_, l)| *l == label.as_ref()).map(|(o, _)| *o).unwrap_or("=")
    }

    fn is_blank(r: &Row, cx: &gpui_kit::App) -> bool {
        r.key.read(cx).value().trim().is_empty() && r.value.read(cx).value().trim().is_empty()
    }

    fn push_row(&mut self, key: String, want: Option<Value>, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.next_id;
        self.next_id += 1;
        let (op, text) = want.as_ref().map(split_matcher).unwrap_or(("=", String::new()));
        let key_state = cx.new(|cx| InputState::new(window, cx).placeholder(self.key_placeholder).default_value(key.clone()));
        let value_state = cx.new(|cx| InputState::new(window, cx).placeholder(value_hint(op)).default_value(text.clone()));
        let op_state = cx.new(|cx| SelectState::new(op_labels(), Some(IndexPath::new(op_index(op))), window, cx));
        for input in [&key_state, &value_state] {
            self._subscriptions.push(cx.subscribe_in(input, window, |this, _, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.ensure_blank_row(window, cx);
                    cx.emit(ExpectTableEvent::Changed);
                    cx.notify();
                }
            }));
        }
        self._subscriptions.push(cx.subscribe_in(&op_state, window, move |this, _, _: &SelectEvent<Vec<SharedString>>, window, cx| {
            if let Some(r) = this.rows.iter().find(|r| r.id == id) {
                let hint = value_hint(this.op_of(r, cx));
                r.value.update(cx, |s, cx| s.set_placeholder(hint, window, cx));
            }
            cx.emit(ExpectTableEvent::Changed);
            cx.notify();
        }));
        self.rows.push(Row { id, key: key_state, op: op_state, value: value_state, original: want.map(|w| (key, op, text, w)) });
    }

    fn ensure_blank_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.rows.last().is_none_or(|r| !Self::is_blank(r, cx)) {
            self.push_row(String::new(), None, window, cx);
        }
    }

    pub fn load(&mut self, map: Option<&Map<String, Value>>, window: &mut Window, cx: &mut Context<Self>) {
        self.rows.clear();
        self._subscriptions.clear();
        for (k, v) in map.into_iter().flatten() {
            self.push_row(k.clone(), Some(v.clone()), window, cx);
        }
        self.ensure_blank_row(window, cx);
        cx.notify();
    }

    /// 追加一条（来自响应「字段」页）：同一路径已有则替换
    pub fn upsert(&mut self, key: &str, want: Value, window: &mut Window, cx: &mut Context<Self>) {
        let mut map = self.to_map_lossy(cx);
        map.insert(key.to_string(), want);
        self.load(Some(&map), window, cx);
        cx.emit(ExpectTableEvent::Changed);
    }

    /// 当前内容 → 文件里的对象；某行的值写法不对时返回带路径的错误
    pub fn to_map(&self, cx: &gpui_kit::App) -> Result<Map<String, Value>, String> {
        let mut out = Map::new();
        for r in &self.rows {
            let key = r.key.read(cx).value().trim().to_string();
            if key.is_empty() {
                continue;
            }
            let op = self.op_of(r, cx);
            let text = r.value.read(cx).value().to_string();
            if let Some((ok, oop, otext, orig)) = &r.original {
                if *ok == key && *oop == op && *otext == text {
                    out.insert(key, orig.clone());
                    continue;
                }
            }
            let want = join_matcher(op, &text).map_err(|e| format!("{} {key}：{e}", self.key_label))?;
            out.insert(key, want);
        }
        Ok(out)
    }

    /// 同 to_map，写法有误的行保留原值（追加时不丢已有内容）
    fn to_map_lossy(&self, cx: &gpui_kit::App) -> Map<String, Value> {
        let mut out = Map::new();
        for r in &self.rows {
            let key = r.key.read(cx).value().trim().to_string();
            if key.is_empty() {
                continue;
            }
            let text = r.value.read(cx).value().to_string();
            let want = join_matcher(self.op_of(r, cx), &text)
                .ok()
                .or_else(|| r.original.as_ref().map(|o| o.3.clone()))
                .unwrap_or(Value::String(text));
            out.insert(key, want);
        }
        out
    }

    fn remove(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.rows.retain(|r| r.id != id);
        self.ensure_blank_row(window, cx);
        cx.emit(ExpectTableEvent::Changed);
        cx.notify();
    }
}

const KEY_W: f32 = 240.;
const OP_W: f32 = 132.;
const ACTION_W: f32 = 40.;

impl Render for ExpectTable {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted, head_bg, radius) = (theme.border, theme.muted_foreground, theme.table_head, theme.radius);
        let last = self.rows.len().saturating_sub(1);
        let cell = move |w: Option<f32>| {
            let d = div().h_full().flex().items_center().border_r_1().border_color(border);
            match w {
                Some(w) => d.w(px(w)).flex_shrink_0(),
                None => d.flex_1().min_w_0(),
            }
        };
        let header = h_flex()
            .h(px(28.))
            .bg(head_bg)
            .border_b_1()
            .border_color(border)
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(muted)
            .child(cell(Some(KEY_W)).px_2().child(self.key_label))
            .child(cell(Some(OP_W)).px_2().child("条件"))
            .child(cell(None).px_2().child("期望值"))
            .child(div().w(px(ACTION_W)).flex_shrink_0());
        let rows = self.rows.iter().enumerate().map(|(ix, r)| {
            let id = r.id;
            let blank_last = ix == last && Self::is_blank(r, cx);
            h_flex()
                .id(("expect-row", id))
                .group("expect-row")
                .h(px(32.))
                .border_b_1()
                .border_color(border)
                .child(cell(Some(KEY_W)).child(div().flex_1().min_w_0().child(Input::new(&r.key).small().appearance(false))))
                .child(cell(Some(OP_W)).child(div().flex_1().min_w_0().child(Select::new(&r.op).small().appearance(false).menu_width(px(180.)))))
                .child(cell(None).child(div().flex_1().min_w_0().child(Input::new(&r.value).small().appearance(false))))
                .child(div().w(px(ACTION_W)).flex_shrink_0().flex().justify_center().when(!blank_last, |this| {
                    this.child(
                        div().invisible().group_hover("expect-row", |d| d.visible()).child(
                            Button::new(("expect-remove", id))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Delete)
                                .tooltip("删除")
                                .on_click(cx.listener(move |this, _, window, cx| this.remove(id, window, cx))),
                        ),
                    )
                }))
        });
        v_flex().rounded(radius).border_1().border_color(border).overflow_hidden().child(header).children(rows)
    }
}
