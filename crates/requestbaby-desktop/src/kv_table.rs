//! 键值表（参照 Postman）：用例请求头 / 查询参数 / 表单 / multipart 与集合请求头共用。
//!
//! - 表格：表头（全选 · 名称 · 值 · 批量编辑），单元格网格线；末尾常驻一行空白行，输入即新增
//! - 每行启用开关：停用的行写成 `{"value": "...", "disabled": true}`，保留在文件里但不发送
//! - 批量编辑：`名称: 值` 每行一个，`//` 开头为停用（与 Postman 一致，可直接粘贴浏览器里的请求头）
//! - multipart：名称格内切换「文本 / 文件」，文件行可从磁盘选择（路径相对用例目录）
//! - 只读行：继承自集合的请求头、自动生成的请求头（Content-Type 等），让最终发送的内容一目了然
//! - 未改动的行原样写回（保留 filename / type 等表格不展示的字段）
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, h_flex,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    input::{Editor, EditorState, Input, InputEvent, InputState},
    v_flex,
};
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, PathPromptOptions,
    Render, SharedString, Styled as _, Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KvMode {
    /// headers / query：带启用开关
    Toggle,
    /// form 表单：纯键值
    Plain,
    /// multipart：文本 / 文件
    Multipart,
}

/// 表格下方的只读行（继承 / 自动生成），不写回文件
#[derive(Debug, Clone)]
pub struct ReadonlyRow {
    pub key: String,
    pub value: String,
    /// 来源说明：「继承自集合 admin」「自动」
    pub source: String,
}

struct Row {
    id: usize,
    enabled: bool,
    file: bool,
    key: Entity<InputState>,
    value: Entity<InputState>,
    /// 加载时的 (key, enabled, file, text, json)：未改动则原样写回
    original: Option<(String, bool, bool, String, Value)>,
}

struct RowInit {
    key: String,
    enabled: bool,
    file: bool,
    text: String,
    original: Option<Value>,
}

pub enum KvTableEvent {
    Changed,
}

pub struct KvTable {
    mode: KvMode,
    /// 名称列表头与空白行占位（「参数名」「Header」「字段名」）
    key_label: &'static str,
    /// 值列表头与占位
    value_label: &'static str,
    rows: Vec<Row>,
    next_id: usize,
    /// 批量编辑文本（Some = 批量编辑模式）
    bulk: Option<Entity<EditorState>>,
    readonly: Vec<ReadonlyRow>,
    /// multipart 文件路径的相对基准（用例所在目录）
    base_dir: Option<PathBuf>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<KvTableEvent> for KvTable {}

fn decode(mode: KvMode, v: &Value) -> (bool, bool, String) {
    match (mode, v) {
        (KvMode::Multipart, Value::Object(o)) => (true, true, o.get("file").and_then(Value::as_str).unwrap_or_default().to_string()),
        (_, Value::Object(o)) => (
            !o.get("disabled").and_then(Value::as_bool).unwrap_or(false),
            false,
            o.get("value").and_then(Value::as_str).unwrap_or_default().to_string(),
        ),
        (_, Value::String(s)) => (true, false, s.clone()),
        (_, other) => (true, false, other.to_string()),
    }
}

/// 批量编辑文本 → (名称, 值, 启用)：`名称: 值`；`//` 开头 = 停用
fn parse_bulk(text: &str) -> Vec<(String, String, bool)> {
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            let (enabled, l) = match l.strip_prefix("//") {
                Some(rest) => (false, rest.trim()),
                None => (true, l),
            };
            let (k, v) = l.split_once(':').unwrap_or((l, ""));
            let k = k.trim();
            (!k.is_empty()).then(|| (k.to_string(), v.trim().to_string(), enabled))
        })
        .collect()
}

impl KvTable {
    pub fn new(mode: KvMode, key_label: &'static str) -> Self {
        Self { mode, key_label, value_label: "值", rows: vec![], next_id: 0, bulk: None, readonly: vec![], base_dir: None, _subscriptions: vec![] }
    }

    /// 自定义值列的表头与占位（捕获表：「来源」）
    pub fn value_label(mut self, label: &'static str) -> Self {
        self.value_label = label;
        self
    }

    /// 设置一项（同名覆盖），保留其余行
    pub fn upsert(&mut self, key: &str, value: Value, window: &mut Window, cx: &mut Context<Self>) {
        let mut map = self.to_map(cx);
        map.insert(key.to_string(), value);
        self.load(Some(&map), window, cx);
        cx.emit(KvTableEvent::Changed);
    }

    pub fn set_base_dir(&mut self, dir: Option<PathBuf>) {
        self.base_dir = dir;
    }

    /// 表格下方的只读行（继承自集合 / 自动生成）
    pub fn set_readonly(&mut self, rows: Vec<ReadonlyRow>, cx: &mut Context<Self>) {
        self.readonly = rows;
        cx.notify();
    }

    fn is_blank(r: &Row, cx: &gpui_kit::App) -> bool {
        r.key.read(cx).value().trim().is_empty() && r.value.read(cx).value().trim().is_empty()
    }

    /// 启用且有名称的行数（标签页计数用）
    pub fn active_count(&self, cx: &gpui_kit::App) -> usize {
        self.entries(cx).iter().filter(|(_, _, on)| *on).count()
    }

    /// 全部值文本（变量引用预览用）
    pub fn texts(&self, cx: &gpui_kit::App) -> String {
        self.entries(cx).into_iter().map(|(_, v, _)| v).collect::<Vec<_>>().join("\n")
    }

    /// 当前内容 (名称, 值, 启用)：批量编辑模式下以文本为准
    fn entries(&self, cx: &gpui_kit::App) -> Vec<(String, String, bool)> {
        if let Some(bulk) = &self.bulk {
            return parse_bulk(&bulk.read(cx).value());
        }
        self.rows
            .iter()
            .filter_map(|r| {
                let k = r.key.read(cx).value().trim().to_string();
                (!k.is_empty()).then(|| (k, r.value.read(cx).value().to_string(), r.enabled))
            })
            .collect()
    }

    fn push_row(&mut self, init: RowInit, window: &mut Window, cx: &mut Context<Self>) {
        let RowInit { key, enabled, file, text, original } = init;
        let id = self.next_id;
        self.next_id += 1;
        let key_state = cx.new(|cx| InputState::new(window, cx).placeholder(self.key_label).default_value(key.clone()));
        let value_ph = if file { "文件路径（相对用例目录）" } else { self.value_label };
        let value_state = cx.new(|cx| InputState::new(window, cx).placeholder(value_ph).default_value(text.clone()));
        for input in [&key_state, &value_state] {
            self._subscriptions.push(cx.subscribe_in(input, window, |this, _, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.ensure_blank_row(window, cx);
                    cx.emit(KvTableEvent::Changed);
                    cx.notify();
                }
            }));
        }
        self.rows.push(Row { id, enabled, file, key: key_state, value: value_state, original: original.map(|o| (key, enabled, file, text, o)) });
    }

    /// 末尾始终保留一行空白行：在里面输入即新增一行（Postman 的交互）
    fn ensure_blank_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.rows.last().is_none_or(|r| !Self::is_blank(r, cx)) {
            self.push_row(RowInit { key: String::new(), enabled: true, file: false, text: String::new(), original: None }, window, cx);
        }
    }

    pub fn load(&mut self, map: Option<&Map<String, Value>>, window: &mut Window, cx: &mut Context<Self>) {
        self.rows.clear();
        self._subscriptions.clear();
        self.bulk = None;
        for (k, v) in map.into_iter().flatten() {
            let (enabled, file, text) = decode(self.mode, v);
            self.push_row(RowInit { key: k.clone(), enabled, file, text, original: Some(v.clone()) }, window, cx);
        }
        self.ensure_blank_row(window, cx);
        cx.notify();
    }

    pub fn to_map(&self, cx: &gpui_kit::App) -> Map<String, Value> {
        let mut out = Map::new();
        if self.bulk.is_some() {
            for (k, v, on) in self.entries(cx) {
                out.insert(k, if self.mode == KvMode::Toggle && !on { json!({ "value": v, "disabled": true }) } else { Value::String(v) });
            }
            return out;
        }
        for r in &self.rows {
            let key = r.key.read(cx).value().trim().to_string();
            if key.is_empty() {
                continue;
            }
            let text = r.value.read(cx).value().to_string();
            if let Some((ok, oen, ofile, otext, orig)) = &r.original {
                if *ok == key && *oen == r.enabled && *ofile == r.file && *otext == text {
                    out.insert(key, orig.clone());
                    continue;
                }
            }
            let v = match self.mode {
                KvMode::Multipart if r.file => json!({ "file": text.trim() }),
                KvMode::Toggle if !r.enabled => json!({ "value": text, "disabled": true }),
                _ => Value::String(text),
            };
            out.insert(key, v);
        }
        out
    }

    fn remove(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.rows.retain(|r| r.id != id);
        self.ensure_blank_row(window, cx);
        cx.emit(KvTableEvent::Changed);
        cx.notify();
    }

    fn toggle(&mut self, id: usize, enabled: bool, cx: &mut Context<Self>) {
        if let Some(r) = self.rows.iter_mut().find(|r| r.id == id) {
            r.enabled = enabled;
        }
        cx.emit(KvTableEvent::Changed);
        cx.notify();
    }

    /// 表头全选框：全部启用 / 全部停用
    fn toggle_all(&mut self, enabled: bool, cx: &mut Context<Self>) {
        for r in self.rows.iter_mut() {
            r.enabled = enabled;
        }
        cx.emit(KvTableEvent::Changed);
        cx.notify();
    }

    fn set_file(&mut self, id: usize, file: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(r) = self.rows.iter_mut().find(|r| r.id == id) {
            r.file = file;
            let ph = if file { "文件路径（相对用例目录）" } else { self.value_label };
            r.value.update(cx, |s, cx| s.set_placeholder(ph, window, cx));
        }
        cx.emit(KvTableEvent::Changed);
        cx.notify();
    }

    /// 表格 ↔ 批量编辑
    fn toggle_bulk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.bulk.take() {
            Some(bulk) => {
                let toggle = self.mode == KvMode::Toggle;
                let map: Map<String, Value> = parse_bulk(&bulk.read(cx).value())
                    .into_iter()
                    .map(|(k, v, on)| (k, if on || !toggle { Value::String(v) } else { json!({ "value": v, "disabled": true }) }))
                    .collect();
                self.load(Some(&map), window, cx);
            }
            None => {
                let text = self
                    .entries(cx)
                    .into_iter()
                    .map(|(k, v, on)| format!("{}{k}: {v}", if on { "" } else { "// " }))
                    .collect::<Vec<_>>()
                    .join("\n");
                let editor = cx.new(|cx| {
                    EditorState::new(window, cx)
                        .soft_wrap(true)
                        .placeholder("每行一个「名称: 值」，// 开头表示停用。可直接粘贴浏览器开发者工具里的请求头".to_string())
                });
                editor.update(cx, |s, cx| s.set_value(text, window, cx));
                self._subscriptions.push(cx.subscribe(&editor, |_, _, ev: &InputEvent, cx| {
                    if matches!(ev, InputEvent::Change) {
                        cx.emit(KvTableEvent::Changed);
                    }
                }));
                self.bulk = Some(editor);
            }
        }
        cx.emit(KvTableEvent::Changed);
        cx.notify();
    }

    /// 选择上传文件：用例目录内写相对路径，其余写绝对路径
    fn pick_file(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("选择上传文件".into()) });
        let base = self.base_dir.clone();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(p) = paths.into_iter().next() else { return };
            let text = relative_to(&p, base.as_deref());
            let _ = this.update_in(cx, |t, window, cx| {
                if let Some(r) = t.rows.iter().find(|r| r.id == id) {
                    r.value.update(cx, |s, cx| s.set_value(text, window, cx));
                    if r.key.read(cx).value().trim().is_empty() {
                        r.key.update(cx, |s, cx| s.set_value("file", window, cx));
                    }
                }
                t.ensure_blank_row(window, cx);
                cx.emit(KvTableEvent::Changed);
                cx.notify();
            });
        })
        .detach();
    }
}

fn relative_to(p: &Path, base: Option<&Path>) -> String {
    match base.and_then(|b| p.strip_prefix(b).ok()) {
        Some(rel) => rel.to_string_lossy().replace('\\', "/"),
        None => p.to_string_lossy().into_owned(),
    }
}

const CHECK_W: f32 = 32.;
const KEY_W: f32 = 240.;
/// 最后一列：表头放「批量编辑」，数据行放悬停删除——宽度一致，列才对得齐
const ACTION_W: f32 = 76.;

impl Render for KvTable {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted, head_bg, radius) = (theme.border, theme.muted_foreground, theme.table_head, theme.radius);
        let mono = theme.mono_font_family.clone();
        let toggle = self.mode == KvMode::Toggle;
        let multipart = self.mode == KvMode::Multipart;
        let in_bulk = self.bulk.is_some();
        let last = self.rows.len().saturating_sub(1);
        let filled: Vec<bool> = self.rows.iter().map(|r| !Self::is_blank(r, cx)).collect();
        let all_on = self.rows.iter().zip(&filled).filter(|(_, f)| **f).all(|(r, _)| r.enabled);
        // 被用例同名启用行覆盖的只读行（名称不区分大小写）
        let own_keys: Vec<String> = self
            .rows
            .iter()
            .filter(|r| r.enabled)
            .map(|r| r.key.read(cx).value().trim().to_lowercase())
            .filter(|k| !k.is_empty())
            .collect();

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
            .when(toggle && !in_bulk, |this| {
                this.child(
                    cell(Some(CHECK_W)).justify_center().child(
                        Checkbox::new("kv-all")
                            .checked(all_on)
                            .tooltip(if all_on { "全部停用" } else { "全部启用" })
                            .on_click(cx.listener(|this, v: &bool, _, cx| this.toggle_all(*v, cx))),
                    ),
                )
            })
            .when(!in_bulk, |this| this.child(cell(Some(KEY_W)).px_2().child(self.key_label)).child(cell(None).px_2().child(self.value_label)))
            .when(in_bulk, |this| this.child(cell(None).px_2().child("每行「名称: 值」，// 开头 = 停用")))
            .child(h_flex().w(px(ACTION_W)).flex_shrink_0().justify_center().when(!multipart, |this| {
                this.child(
                    Button::new("kv-bulk")
                        .ghost()
                        .xsmall()
                        .label(if in_bulk { "表格编辑" } else { "批量编辑" })
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_bulk(window, cx))),
                )
            }));

        let body = match &self.bulk {
            Some(editor) => div().h(px(200.)).child(Editor::new(editor).h_full()).into_any_element(),
            None => v_flex()
                .children(self.rows.iter().enumerate().map(|(ix, r)| {
                    let id = r.id;
                    let (enabled, file) = (r.enabled, r.file);
                    let blank_last = ix == last && !filled[ix];
                    h_flex()
                        .id(("kv-row", id))
                        .group("kv-row")
                        .h(px(32.))
                        .border_b_1()
                        .border_color(border)
                        .when(toggle, |this| {
                            this.child(cell(Some(CHECK_W)).justify_center().when(!blank_last, |c| {
                                c.child(
                                    Checkbox::new(("kv-on", id))
                                        .checked(enabled)
                                        .tooltip(if enabled { "已启用：取消勾选则保留但不发送" } else { "已停用：保留在用例里但不发送" })
                                        .on_click(cx.listener(move |this, checked: &bool, _, cx| this.toggle(id, *checked, cx))),
                                )
                            }))
                        })
                        .child(
                            cell(Some(KEY_W))
                                .when(!enabled, |d| d.opacity(0.45))
                                .child(div().flex_1().min_w_0().child(Input::new(&r.key).small().appearance(false)))
                                .when(multipart && !blank_last, |this| {
                                    this.child(
                                        Button::new(("kv-kind", id))
                                            .xsmall()
                                            .ghost()
                                            .label(if file { "文件" } else { "文本" })
                                            .icon(IconName::ChevronDown)
                                            .tooltip("切换文本字段 / 文件字段")
                                            .on_click(cx.listener(move |this, _, window, cx| this.set_file(id, !file, window, cx))),
                                    )
                                }),
                        )
                        .child(
                            cell(None)
                                .when(!enabled, |d| d.opacity(0.45))
                                .child(div().flex_1().min_w_0().child(Input::new(&r.value).small().appearance(false)))
                                .when(multipart && file, |this| {
                                    this.child(
                                        Button::new(("kv-pick", id))
                                            .xsmall()
                                            .ghost()
                                            .icon(IconName::FolderOpen)
                                            .label("选择文件")
                                            .on_click(cx.listener(move |this, _, window, cx| this.pick_file(id, window, cx))),
                                    )
                                }),
                        )
                        .child(div().w(px(ACTION_W)).flex_shrink_0().flex().justify_center().when(!blank_last, |this| {
                            this.child(
                                div().invisible().group_hover("kv-row", |d| d.visible()).child(
                                    Button::new(("kv-remove", id))
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Delete)
                                        .tooltip("删除")
                                        .on_click(cx.listener(move |this, _, window, cx| this.remove(id, window, cx))),
                                ),
                            )
                        }))
                }))
                .into_any_element(),
        };

        // 只读行：继承 / 自动生成（最终会一起发送的内容）
        let readonly: Vec<_> = self
            .readonly
            .iter()
            .enumerate()
            .map(|(ix, r)| {
                let overridden = own_keys.contains(&r.key.to_lowercase());
                h_flex()
                    .id(("kv-ro", ix))
                    .h(px(28.))
                    .border_b_1()
                    .border_color(border)
                    .text_xs()
                    .text_color(muted)
                    .when(toggle, |this| this.child(cell(Some(CHECK_W)).justify_center().child(Icon::new(IconName::Info).xsmall())))
                    .child(cell(Some(KEY_W)).px_2().font_family(mono.clone()).when(overridden, |d| d.line_through()).child(r.key.clone()))
                    .child(
                        cell(None)
                            .px_2()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .font_family(mono.clone())
                                    .when(overridden, |d| d.line_through())
                                    .child(r.value.clone()),
                            )
                            .child(div().flex_shrink_0().child(SharedString::from(if overridden {
                                format!("{} · 已被上方同名覆盖", r.source)
                            } else {
                                r.source.clone()
                            }))),
                    )
                    .child(div().w(px(ACTION_W)).flex_shrink_0())
            })
            .collect();

        v_flex()
            .rounded(radius)
            .border_1()
            .border_color(border)
            .overflow_hidden()
            .child(header)
            .child(body)
            .when(!readonly.is_empty() && !in_bulk, |this| this.children(readonly))
    }
}

#[cfg(test)]
mod tests {
    use super::parse_bulk;

    #[test]
    fn bulk_text_parses_like_postman() {
        let rows = parse_bulk("Authorization: Bearer {{token}}\n// X-Debug: 1\n\nAccept:application/json\nbad-line");
        assert_eq!(
            rows,
            vec![
                ("Authorization".into(), "Bearer {{token}}".into(), true),
                ("X-Debug".into(), "1".into(), false),
                ("Accept".into(), "application/json".into(), true),
                ("bad-line".into(), "".into(), true),
            ]
        );
    }
}
