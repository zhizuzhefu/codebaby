//! 响应面板：状态 / 响应体 / 字段（一键断言 / 捕获）/ 响应头 / 断言 / cURL / 脚本日志。
//! gRPC 显示状态名、响应消息与元数据、等价 grpcurl；WebSocket 多一个「对话」页，按时间列出收发的每条消息；
//! Web / Android 多一个「步骤」页：每一步做了什么、结果、截图（点击放大）、运行环境与失败现场。
//!
//! 数据来自两处：Desktop 本次运行的完整结果（含完整响应体），
//! 或共享历史中的协议结果（失败用例带 4KB 响应快照）——AI 在 CLI 里跑挂的用例同样能看到现场。
//!
//! 复制约定（全面板一致）：**点击即复制，原位显示「已复制」**；右键给出其他格式（路径 / cURL / 「名称: 值」…）；
//! 响应体是只读编辑器，选中片段可直接 ⌘C。
use crate::step_viewer::{StepViewer, ViewerStep};
use crate::style;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, h_flex,
    button::{Button, ButtonGroup, ButtonVariants as _},
    input::{Editor, EditorState, Input, InputEvent, InputState},
    menu::{ContextMenuExt as _, PopupMenuItem},
    notification::Notification,
    tab::{Tab, TabBar},
    tag::Tag,
    tooltip::Tooltip,
    WindowExt as _,
    v_flex,
};
use gpui_kit::{
    AppContext as _, ClipboardItem, Context, Entity, Focusable as _, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task, WeakEntity, Window, div,
    prelude::FluentBuilder as _, px,
};
use requestbaby_core::config::Protocol;
use requestbaby_core::engine::Frame;
use requestbaby_core::report::{CaseReport, TestReport};
use requestbaby_core::runner::CaseResult;
use requestbaby_core::suggest::{self, Addition, JsonField};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RespTab {
    Steps,
    Dialog,
    Body,
    Fields,
    Headers,
    Tests,
    Curl,
    Logs,
}

impl RespTab {
    const ALL: [RespTab; 8] = [RespTab::Steps, RespTab::Dialog, RespTab::Body, RespTab::Fields, RespTab::Headers, RespTab::Tests, RespTab::Curl, RespTab::Logs];
}

/// 面板展示所需的统一视图（来自完整结果或历史协议结果）
#[derive(Debug, Clone, Default)]
struct Shown {
    /// 用例文件（相对项目根）：字段页「添加断言 / 捕获」写回它
    file: String,
    method: String,
    url: String,
    protocol: Protocol,
    /// 拿到了响应（gRPC 的 OK 是 0，不能用 status 判断）
    responded: bool,
    /// 消息时间线（WebSocket 收发 / gRPC 请求与响应消息）
    frames: Vec<Frame>,
    /// 界面用例的证据（每一步的记录与截图）
    evidence: Option<requestbaby_core::engine::ui::UiEvidence>,
    status: u16,
    duration_ms: u64,
    size: Option<u64>,
    skipped: bool,
    passed: bool,
    error: Option<String>,
    headers: Vec<(String, String)>,
    body: Option<String>,
    /// 响应体是 JSON 时的格式化文本（展示与复制共用，只计算一次）
    body_pretty: Option<String>,
    body_truncated: bool,
    /// 二进制响应的原始字节（仅本次发送有；历史只记录大小）
    raw: Option<Arc<Vec<u8>>>,
    tests: Vec<TestReport>,
    curl: Option<String>,
    logs: Vec<String>,
    /// 来源说明：本次发送 / 历史运行 id
    origin: String,
}

/// 字段页的写回请求（由工作区写入用例：编辑器已打开则进编辑器，否则直接写文件）
pub enum ResponsePanelEvent {
    /// 写进 expect.body 的一条断言或 capture 的一个变量
    Add { file: String, add: Addition },
}

impl gpui_kit::EventEmitter<ResponsePanelEvent> for ResponsePanel {}

pub struct ResponsePanel {
    shown: Option<Shown>,
    /// 响应 JSON 展开的字段（随 shown 计算一次）
    fields: Vec<JsonField>,
    field_filter: Entity<InputState>,
    tab: RespTab,
    body: Entity<EditorState>,
    /// 响应体按原始文本展示（默认 JSON 格式化）
    raw_body: bool,
    /// 项目根：界面用例的截图路径相对它
    root: Option<std::path::PathBuf>,
    /// 刚复制的目标（原位显示「已复制」）
    copied: Option<SharedString>,
    _copied_reset: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

fn pretty(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body).ok().and_then(|v| serde_json::to_string_pretty(&v).ok())
}

/// URL 去掉 scheme + host，保留路径与查询（粘贴到代码 / 路由表里用）
fn path_and_query(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    match rest.find('/') {
        Some(i) => rest[i..].to_string(),
        None => "/".into(),
    }
}

impl ResponsePanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let body = cx.new(|cx| EditorState::new(window, cx).language("json".to_string()).line_number(true).soft_wrap(true));
        let field_filter = cx.new(|cx| InputState::new(window, cx).placeholder("筛选字段路径 / 值").clean_on_escape());
        let sub = cx.subscribe(&field_filter, |_, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                cx.notify();
            }
        });
        Self {
            shown: None,
            fields: vec![],
            field_filter,
            tab: RespTab::Body,
            body,
            raw_body: false,
            root: None,
            copied: None,
            _copied_reset: None,
            _subscriptions: vec![sub],
        }
    }

    /// 复制并在原位置给出「已复制」反馈（1.5s 后恢复）
    fn copy(&mut self, key: &str, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied = Some(SharedString::from(key.to_string()));
        self._copied_reset = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1500)).await;
            let _ = this.update(cx, |s, cx| {
                s.copied = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn is_copied(&self, key: &str) -> bool {
        self.copied.as_deref() == Some(key)
    }

    /// 当前展示的响应体文本（格式化 / 原始）
    fn body_text(&self) -> Option<String> {
        let s = self.shown.as_ref()?;
        let raw = s.body.clone()?;
        Some(if self.raw_body { raw } else { s.body_pretty.clone().unwrap_or(raw) })
    }

    fn set_raw_body(&mut self, raw: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.raw_body = raw;
        let text = self.body_text().unwrap_or_default();
        self.body.update(cx, |s, cx| s.set_value(text, window, cx));
        cx.notify();
    }


    /// 项目根（截图等证据文件相对它）
    pub fn set_root(&mut self, root: Option<std::path::PathBuf>) {
        self.root = root;
    }

    pub fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shown = None;
        self.body.update(cx, |s, cx| s.set_value("", window, cx));
        cx.notify();
    }

    /// 本次 Desktop 运行的完整结果
    pub fn show_result(&mut self, r: &CaseResult, origin: &str, window: &mut Window, cx: &mut Context<Self>) {
        let resp = r.full_response.as_ref();
        let shown = Shown {
            file: r.rel.clone(),
            method: r.method.clone(),
            url: r.url.clone(),
            protocol: r.protocol,
            responded: r.responded(),
            frames: resp.map(|x| x.frames.clone()).unwrap_or_default(),
            evidence: resp.and_then(|x| x.evidence.clone()).or_else(|| r.response_snapshot.as_ref().and_then(|s| s.evidence.clone())),
            status: r.status,
            duration_ms: r.duration_ms,
            size: resp.map(|x| x.size),
            skipped: r.skipped,
            passed: r.passed(),
            error: r.error.clone(),
            headers: resp.map(|x| x.headers.clone()).unwrap_or_default(),
            body: resp.map(|x| x.body.clone()),
            body_pretty: None,
            body_truncated: false,
            raw: r.raw_body.clone(),
            tests: r.tests.iter().map(|t| TestReport { expr: t.expr.clone(), pass: t.pass, detail: t.detail.clone() }).collect(),
            curl: r.curl_cmd.clone(),
            logs: r.logs.clone(),
            origin: origin.to_string(),
        };
        self.set(shown, window, cx);
    }

    /// 共享历史中的协议结果（可能来自 CLI / CI / AI）
    pub fn show_report(&mut self, r: &CaseReport, origin: &str, window: &mut Window, cx: &mut Context<Self>) {
        let snap = r.response.as_ref();
        let shown = Shown {
            file: r.file.clone(),
            method: r.method.clone(),
            url: r.url.clone(),
            protocol: r.protocol,
            responded: snap.is_some() || (matches!(r.protocol, Protocol::Http | Protocol::Ws) && r.status > 0),
            frames: snap.map(|s| s.frames.clone()).unwrap_or_default(),
            evidence: snap.and_then(|s| s.evidence.clone()),
            status: r.status,
            duration_ms: r.duration_ms,
            size: None,
            skipped: r.skipped,
            passed: r.passed,
            error: r.error.clone(),
            headers: snap.map(|s| s.headers.clone()).unwrap_or_default(),
            body: snap.map(|s| s.body.clone()),
            body_pretty: None,
            body_truncated: snap.is_some_and(|s| s.truncated),
            raw: None,
            tests: r.tests.clone(),
            curl: r.curl.clone(),
            logs: r.logs.clone(),
            origin: origin.to_string(),
        };
        self.set(shown, window, cx);
    }

    fn set(&mut self, mut shown: Shown, window: &mut Window, cx: &mut Context<Self>) {
        shown.body_pretty = shown.body.as_deref().and_then(pretty);
        self.fields = if shown.body_truncated || shown.raw.is_some() {
            vec![]
        } else {
            shown.body.as_deref().map(|b| suggest::fields(b, 20, 500)).unwrap_or_default()
        };
        if self.tab == RespTab::Fields && self.fields.is_empty() {
            self.tab = RespTab::Body;
        }
        // 失败时直接把视线引到断言；WebSocket 默认看对话
        let failed_assertions = !shown.passed && !shown.skipped && !shown.tests.is_empty() && shown.error.is_none();
        if failed_assertions || (shown.body.is_none() && !shown.tests.is_empty()) {
            self.tab = RespTab::Tests;
        } else if self.tab == RespTab::Tests && shown.tests.is_empty() {
            self.tab = RespTab::Body;
        }
        if shown.protocol == Protocol::Ws && !shown.frames.is_empty() && !failed_assertions && matches!(self.tab, RespTab::Body | RespTab::Tests) {
            self.tab = RespTab::Dialog;
        }
        if self.tab == RespTab::Dialog && !Self::has_dialog(&shown) {
            self.tab = RespTab::Body;
        }
        // 界面用例：先看步骤与截图（失败的那一步就在里面）
        if shown.evidence.is_some() {
            self.tab = RespTab::Steps;
        } else if self.tab == RespTab::Steps {
            self.tab = if failed_assertions { RespTab::Tests } else { RespTab::Body };
        }
        self.shown = Some(shown);
        let text = self.body_text().unwrap_or_default();
        self.body.update(cx, |s, cx| s.set_value(text, window, cx));
        cx.notify();
    }

    /// 有消息时间线可看：WebSocket，或 gRPC 的流式调用
    fn has_dialog(s: &Shown) -> bool {
        match s.protocol {
            Protocol::Ws => !s.frames.is_empty(),
            Protocol::Grpc => s.frames.len() > 2,
            Protocol::Http | Protocol::Web | Protocol::Android => false,
        }
    }

    fn visible_tabs(&self) -> Vec<RespTab> {
        let dialog = self.shown.as_ref().is_some_and(Self::has_dialog);
        let ui = self.shown.as_ref().is_some_and(|s| s.protocol.is_ui());
        let steps = self.shown.as_ref().is_some_and(|s| s.evidence.is_some());
        RespTab::ALL
            .iter()
            .copied()
            .filter(|t| *t != RespTab::Dialog || dialog)
            .filter(|t| *t != RespTab::Steps || steps)
            // 界面用例没有字段 / 等价命令可看
            .filter(|t| !ui || !matches!(t, RespTab::Fields | RespTab::Curl))
            .collect()
    }

    fn tab_label(&self, tab: RespTab) -> String {
        let s = self.shown.as_ref();
        let protocol = s.map(|s| s.protocol).unwrap_or_default();
        match tab {
            RespTab::Steps => match s.and_then(|s| s.evidence.as_ref()) {
                Some(ev) => format!("步骤 {}", ev.steps.len()),
                None => "步骤".into(),
            },
            RespTab::Dialog => match s.map(|s| s.frames.len()).filter(|n| *n > 0) {
                Some(n) => format!("对话 {n}"),
                None => "对话".into(),
            },
            RespTab::Body => match protocol {
                Protocol::Http => "响应体".into(),
                Protocol::Grpc => "响应消息".into(),
                Protocol::Ws if s.is_some_and(|s| s.status != 101) => "握手响应体".into(),
                Protocol::Ws => "消息".into(),
                Protocol::Web => "页面".into(),
                Protocol::Android => "界面".into(),
            },
            RespTab::Fields => match self.fields.len() {
                0 => "字段".into(),
                n => format!("字段 {n}"),
            },
            RespTab::Headers => {
                let title = match protocol {
                    Protocol::Http => "响应头",
                    Protocol::Grpc => "元数据",
                    Protocol::Ws => "握手头",
                    Protocol::Web | Protocol::Android => "页面响应头",
                };
                match s.map(|s| s.headers.len()).filter(|n| *n > 0) {
                    Some(n) => format!("{title} {n}"),
                    None => title.into(),
                }
            }
            RespTab::Tests => match s.filter(|s| !s.tests.is_empty()) {
                Some(s) => format!("断言 {}/{}", s.tests.iter().filter(|t| t.pass).count(), s.tests.len()),
                None => "断言".into(),
            },
            RespTab::Curl if protocol == Protocol::Http => "cURL".into(),
            RespTab::Curl => "命令".into(),
            RespTab::Logs => match s.map(|s| s.logs.len()).filter(|n| *n > 0) {
                Some(n) => format!("日志 {n}"),
                None => "日志".into(),
            },
        }
    }

    // ------------------------------------------------------------ 二进制响应

    fn content_type(s: &Shown) -> String {
        s.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.split(';').next().unwrap_or("").trim().to_ascii_lowercase())
            .unwrap_or_default()
    }

    /// 另存为：建议文件名取 Content-Disposition filename，其次 URL 末段 + 扩展名
    fn save_binary(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = &self.shown else { return };
        let Some(bytes) = s.raw.clone() else { return };
        let disposition = s.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-disposition")).map(|(_, v)| v.clone());
        let from_disposition = disposition.and_then(|d| d.split("filename=").nth(1).map(|f| f.trim_matches(['"', ' ', ';']).to_string()));
        let ext = gpui_kit::ImageFormat::from_mime_type(&Self::content_type(s)).map(|f| f.extension()).unwrap_or("bin");
        let from_url = path_and_query(&s.url).split(['?', '#']).next().unwrap_or("").rsplit('/').next().unwrap_or("").to_string();
        let name = from_disposition.filter(|f| !f.is_empty()).unwrap_or_else(|| {
            if from_url.contains('.') { from_url } else { format!("{}.{ext}", if from_url.is_empty() { "response" } else { &from_url }) }
        });
        let dir = dirs_download();
        let rx = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(path))) = rx.await else { return };
            let note = match requestbaby_core::fsx::write(&path, bytes.as_slice()) {
                Ok(()) => Notification::success(format!("已保存：{}", path.display())),
                Err(e) => Notification::error(format!("保存失败：{e}")),
            };
            let _ = cx.update(|window, cx| window.push_notification(note, cx));
        })
        .detach();
    }

    fn render_binary(&self, s: &Shown, cx: &mut Context<Self>) -> impl IntoElement {
        let raw = s.raw.clone().unwrap_or_default();
        let ct = Self::content_type(s);
        let format = gpui_kit::ImageFormat::from_mime_type(&ct);
        let label = format!("二进制 · {} · {}", if ct.is_empty() { "未知类型" } else { &ct }, human_size(raw.len() as u64));
        v_flex()
            .flex_1()
            .min_h_0()
            .p_3()
            .gap_2()
            .child(
                self.toolbar(label, cx).child(
                    Button::new("save-binary")
                        .xsmall()
                        .ghost()
                        .icon(IconName::HardDrive)
                        .label("另存为…")
                        .on_click(cx.listener(|this, _, window, cx| this.save_binary(window, cx))),
                ),
            )
            .child(match format {
                Some(f) => div()
                    .id("binary-preview")
                    .flex_1()
                    .min_h_0()
                    .overflow_scroll()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().muted)
                    .p_2()
                    .child(gpui_kit::img(Arc::new(gpui_kit::Image::from_bytes(f, raw.as_ref().clone()))).max_w_full())
                    .into_any_element(),
                None => empty_hint("非图片的二进制内容（文件下载 / PDF / 压缩包…）：用「另存为…」保存后查看", cx).into_any_element(),
            })
    }

    // ------------------------------------------------------------ 字段 → 断言 / 捕获

    fn emit_add(&mut self, add: Addition, cx: &mut Context<Self>) {
        if let Some(s) = &self.shown {
            cx.emit(ResponsePanelEvent::Add { file: s.file.clone(), add });
        }
    }

    fn capture(&mut self, field: JsonField, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file) = self.shown.as_ref().map(|s| s.file.clone()) else { return };
        let view = cx.entity().downgrade();
        crate::dialogs::prompt_text(
            window,
            cx,
            "捕获为变量",
            format!("写入用例的 capture：<名称> ← {}；后续用例用 {{{{名称}}}} 引用", field.path),
            field.var_name(),
            "添加",
            move |name, _, cx| {
                if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    anyhow::bail!("变量名只能包含字母、数字、下划线");
                }
                let (file, add) = (file.clone(), field.capture(&name));
                let _ = view.update(cx, |_, cx| cx.emit(ResponsePanelEvent::Add { file, add }));
                Ok(())
            },
        );
    }

    fn render_fields(&self, s: &Shown, cx: &mut Context<Self>) -> impl IntoElement {
        if self.fields.is_empty() {
            let hint = if s.body_truncated {
                "历史快照被截断，无法可靠展开；点击「发送」获取完整响应后再生成断言"
            } else {
                "响应体不是 JSON：可在「断言」页对路径 $ 用「包含」检查文本"
            };
            return empty_hint(hint, cx).into_any_element();
        }
        let muted = cx.theme().muted_foreground;
        let mono = cx.theme().mono_font_family.clone();
        let query = self.field_filter.read(cx).value().trim().to_lowercase();
        let rows = self
            .fields
            .iter()
            .enumerate()
            .filter(|(_, f)| query.is_empty() || f.path.to_lowercase().contains(&query) || f.preview().to_lowercase().contains(&query))
            .map(|(ix, f)| {
                let key = format!("field:{ix}");
                let (f1, f2, f3, path) = (f.clone(), f.clone(), f.clone(), f.path.clone());
                h_flex()
                    .id(("resp-field", ix))
                    .group("resp-field")
                    .gap_2()
                    .px_1()
                    .py_0p5()
                    .text_xs()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .hover(|d| d.bg(cx.theme().muted))
                    .child(
                        div()
                            .id(("field-path", ix))
                            .w(px(260.))
                            .flex_shrink_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .font_family(mono.clone())
                            .cursor_pointer()
                            .child(f.path.clone())
                            .tooltip({
                                let p = f.path.clone();
                                move |window, cx| Tooltip::new(format!("{p}（点击复制路径）")).build(window, cx)
                            })
                            .on_click(cx.listener({
                                let key = key.clone();
                                move |this, _, _, cx| this.copy(&key, path.clone(), cx)
                            })),
                    )
                    .child(div().w(px(56.)).flex_shrink_0().text_color(muted).child(f.kind()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .font_family(mono.clone())
                            .child(f.preview()),
                    )
                    .child(self.copy_mark(&key, "resp-field", cx))
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .gap_1()
                            .invisible()
                            .group_hover("resp-field", |d| d.visible())
                            .child(
                                Button::new(("field-assert", ix))
                                    .xsmall()
                                    .ghost()
                                    .label("断言值")
                                    .tooltip(format!("expect.body：{}", f1.value_expect().snippet()))
                                    .on_click(cx.listener(move |this, _, _, cx| this.emit_add(f1.value_expect(), cx))),
                            )
                            .child(
                                Button::new(("field-type", ix))
                                    .xsmall()
                                    .ghost()
                                    .label("断言类型")
                                    .tooltip(format!("expect.body：{}（值每次会变时用，如 id / 时间 / token）", f2.type_expect().snippet()))
                                    .on_click(cx.listener(move |this, _, _, cx| this.emit_add(f2.type_expect(), cx))),
                            )
                            .child(
                                Button::new(("field-capture", ix))
                                    .xsmall()
                                    .ghost()
                                    .label("捕获…")
                                    .tooltip("写入 capture：<名称> ← 该字段，供后续用例 {{名称}} 引用")
                                    .on_click(cx.listener(move |this, _, window, cx| this.capture(f3.clone(), window, cx))),
                            ),
                    )
            });
        v_flex()
            .flex_1()
            .min_h_0()
            .p_3()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(div().w(px(240.)).child(Input::new(&self.field_filter).xsmall().cleanable(true)))
                    .child(div().text_xs().text_color(muted).child(format!("{} · 悬停一行：断言值 / 断言类型 / 捕获为变量，写入 {}", self.fields.len(), s.file))),
            )
            .child(v_flex().id("resp-fields").flex_1().min_h_0().overflow_y_scroll().children(rows))
            .into_any_element()
    }

    // ------------------------------------------------------------ 复制组件

    /// 行尾复制标记：刚复制显示「✓ 已复制」，否则悬停该行（group）时显示复制图标
    fn copy_mark(&self, key: &str, group: &'static str, cx: &Context<Self>) -> impl IntoElement {
        if self.is_copied(key) {
            h_flex()
                .flex_shrink_0()
                .gap_1()
                .text_xs()
                .text_color(cx.theme().success)
                .child(Icon::new(IconName::Check).xsmall())
                .child("已复制")
                .into_any_element()
        } else {
            div()
                .flex_shrink_0()
                .invisible()
                .group_hover(group, |s| s.visible())
                .child(Icon::new(IconName::Copy).xsmall().text_color(cx.theme().muted_foreground))
                .into_any_element()
        }
    }

    /// 工具栏复制按钮：复制内容在点击时才取（响应体可能很大，不随每次渲染克隆）
    fn copy_button(
        &self,
        id: &'static str,
        label: &'static str,
        get: impl Fn(&Self) -> Option<String> + 'static,
        cx: &mut Context<Self>,
    ) -> Button {
        let copied = self.is_copied(id);
        Button::new(id)
            .xsmall()
            .ghost()
            .icon(if copied { IconName::Check } else { IconName::Copy })
            .label(if copied { "已复制" } else { label })
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(text) = get(this) {
                    this.copy(id, text, cx);
                }
            }))
    }

    fn toolbar(&self, hint: impl IntoElement, cx: &Context<Self>) -> gpui_kit::Div {
        h_flex().gap_2().text_xs().text_color(cx.theme().muted_foreground).child(hint).child(div().flex_1())
    }

    // ------------------------------------------------------------ 摘要

    fn render_summary(&self, s: &Shown, cx: &mut Context<Self>) -> impl IntoElement {
        let text = requestbaby_core::runner::status_text(s.protocol, s.status, s.responded);
        let status_tag = if s.skipped {
            Tag::secondary().xsmall().child("SKIPPED")
        } else if !s.responded {
            Tag::danger().xsmall().child("无响应")
        } else if requestbaby_core::runner::status_is_bad(s.protocol, s.status, s.responded) {
            Tag::danger().xsmall().child(text)
        } else if s.protocol == Protocol::Http && s.status >= 300 {
            Tag::warning().xsmall().child(text)
        } else {
            Tag::success().xsmall().child(text)
        };
        let verdict = if s.skipped {
            (IconName::Ban, cx.theme().muted_foreground, "跳过")
        } else if s.passed {
            (IconName::CircleCheck, cx.theme().success, "通过")
        } else {
            (IconName::CircleX, cx.theme().danger, "失败")
        };
        let mut meta = format!("{} ms", s.duration_ms);
        if let Some(size) = s.size {
            meta.push_str(&format!(" · {}", human_size(size)));
        }
        let view = cx.entity().downgrade();
        let (method, url, curl, protocol) = (s.method.clone(), s.url.clone(), s.curl.clone(), s.protocol);
        let tip = format!("{url}\n点击复制地址 · 右键：路径 / 带方法 / {}", command_name(protocol));
        let url_row = h_flex()
            .id("resp-url")
            .group("resp-url")
            .gap_2()
            .px_1()
            .py_0p5()
            .rounded(cx.theme().radius)
            .cursor_pointer()
            .hover(|d| d.bg(cx.theme().muted))
            .text_xs()
            .child(style::method_label(&method, cx).flex_shrink_0())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_color(cx.theme().foreground)
                    .child(url.clone()),
            )
            .child(self.copy_mark("url", "resp-url", cx))
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(url) = this.shown.as_ref().map(|s| s.url.clone()) {
                    this.copy("url", url, cx);
                }
            }))
            .context_menu(move |menu, _, _| {
                let mut menu = menu
                    .item(copy_item(&view, "复制 URL", "url", url.clone()))
                    .item(copy_item(&view, "复制路径（不含域名）", "url", path_and_query(&url)))
                    .item(copy_item(&view, format!("复制「{method} URL」"), "url", format!("{method} {url}")));
                if let Some(c) = &curl {
                    menu = menu.item(copy_item(&view, format!("复制为 {} 命令", command_name(protocol)), "url", c.clone()));
                }
                if protocol == Protocol::Http && method.eq_ignore_ascii_case("GET") {
                    let url = url.clone();
                    menu = menu.separator().item(
                        PopupMenuItem::new("在浏览器中打开").icon(IconName::ExternalLink).on_click(move |_, _, cx| cx.open_url(&url)),
                    );
                }
                menu
            });
        v_flex()
            .px_4()
            .py_3()
            .gap_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .gap_2()
                    .child(Icon::new(verdict.0).small().text_color(verdict.1))
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(verdict.2))
                    .child(status_tag)
                    .child(div().text_xs().text_color(cx.theme().muted_foreground).child(meta))
                    .child(div().flex_1())
                    .child(div().text_xs().text_color(cx.theme().muted_foreground).child(s.origin.clone())),
            )
            .child(div().mx(px(-4.)).child(url_row))
            .when_some(s.error.clone(), |this, e| {
                this.child(
                    h_flex()
                        .id("resp-error")
                        .group("resp-error")
                        .mt_1()
                        .gap_2()
                        .items_start()
                        .px_1()
                        .rounded(cx.theme().radius)
                        .cursor_pointer()
                        .hover(|d| d.bg(cx.theme().muted))
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(Icon::new(IconName::TriangleAlert).xsmall())
                        .child(div().flex_1().min_w_0().child(e.clone()))
                        .child(self.copy_mark("error", "resp-error", cx))
                        .tooltip(|window, cx| Tooltip::new("点击复制错误信息").build(window, cx))
                        .on_click(cx.listener(move |this, _, _, cx| this.copy("error", e.clone(), cx))),
                )
            })
    }

    // ------------------------------------------------------------ 标签页

    fn render_tab(&self, s: &Shown, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let mono = cx.theme().mono_font_family.clone();
        let view = cx.entity().downgrade();
        match self.tab {
            RespTab::Body if s.raw.is_some() => self.render_binary(s, cx).into_any_element(),
            // gRPC 非 OK 没有响应消息：直接给出状态与服务端说明
            RespTab::Body if s.protocol == Protocol::Grpc && s.responded && s.status != 0 && s.body.as_deref().is_none_or(|b| b.trim() == "null") => {
                let text = requestbaby_core::runner::status_text(s.protocol, s.status, true);
                let msg = s.headers.iter().find(|(k, _)| k == "grpc-message").map(|(_, v)| format!("：{v}")).unwrap_or_default();
                empty_hint(format!("服务端返回 {text}{msg}；没有响应消息（说明在「元数据」的 grpc-message）"), cx).into_any_element()
            }
            RespTab::Body => match &s.body {
                Some(body) => {
                    let is_json = s.body_pretty.is_some();
                    let size = s.size.unwrap_or(body.len() as u64);
                    let kind = if is_json { "JSON" } else { "文本" };
                    v_flex()
                        .flex_1()
                        .min_h_0()
                        .p_3()
                        .gap_2()
                        .child(
                            self.toolbar(format!("{kind} · {} · 选中片段 ⌘C 复制", human_size(size)), cx)
                                .when(is_json, |this| {
                                    this.child(
                                        ButtonGroup::new("body-mode")
                                            .xsmall()
                                            .outline()
                                            .compact()
                                            .child(Button::new("body-pretty").label("格式化").selected(!self.raw_body))
                                            .child(Button::new("body-raw").label("原始").selected(self.raw_body))
                                            .on_click(cx.listener(|this, clicks: &Vec<usize>, window, cx| {
                                                if let Some(ix) = clicks.first() {
                                                    this.set_raw_body(*ix == 1, window, cx);
                                                }
                                            })),
                                    )
                                })
                                .child(self.copy_button("copy-body", "复制响应体", |this| this.body_text(), cx)),
                        )
                        .when(s.body_truncated, |this| {
                            this.child(div().text_xs().text_color(cx.theme().warning).child("历史快照只保留了响应的前一部分；点击「发送」获取完整响应"))
                        })
                        .child(div().flex_1().min_h_0().child(Editor::new(&self.body).readonly(true).bordered(false).h_full()))
                        .into_any_element()
                }
                None => empty_hint(
                    if !s.responded { "请求没有拿到响应（见上方错误）" } else { "没有响应快照（较早版本的历史只为失败用例保存）；点击「发送」查看完整响应" },
                    cx,
                )
                .into_any_element(),
            },
            RespTab::Fields => self.render_fields(s, cx).into_any_element(),
            RespTab::Dialog => self.render_dialog(s, cx).into_any_element(),
            RespTab::Steps => self.render_steps(s, cx).into_any_element(),
            RespTab::Headers => {
                if s.headers.is_empty() {
                    return empty_hint(if s.protocol == Protocol::Grpc { "没有元数据" } else { "没有响应头" }, cx).into_any_element();
                }
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .p_3()
                    .gap_2()
                    .child(
                        self.toolbar(format!("{} 个响应头 · 点击复制值，右键复制名称或整行", s.headers.len()), cx).child(self.copy_button(
                            "copy-headers",
                            "复制全部",
                            |this| this.shown.as_ref().map(|s| s.headers.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("\n")),
                            cx,
                        )),
                    )
                    .child(
                        v_flex().id("resp-headers").flex_1().min_h_0().overflow_y_scroll().children(s.headers.iter().enumerate().map(
                            |(ix, (k, v))| {
                                let key = format!("header:{ix}");
                                let (k, v, view) = (k.clone(), v.clone(), view.clone());
                                h_flex()
                                    .id(("resp-header", ix))
                                    .group("resp-header")
                                    .gap_3()
                                    .px_1()
                                    .py_1()
                                    .items_start()
                                    .text_xs()
                                    .border_b_1()
                                    .border_color(cx.theme().border)
                                    .cursor_pointer()
                                    .hover(|d| d.bg(cx.theme().muted))
                                    .child(div().w(px(180.)).flex_shrink_0().font_weight(FontWeight::MEDIUM).child(k.clone()))
                                    .child(div().flex_1().min_w_0().font_family(mono.clone()).text_color(muted).child(v.clone()))
                                    .child(self.copy_mark(&key, "resp-header", cx))
                                    .on_click(cx.listener({
                                        let (key, v) = (key.clone(), v.clone());
                                        move |this, _, _, cx| this.copy(&key, v.clone(), cx)
                                    }))
                                    .context_menu(move |menu, _, _| {
                                        menu.item(copy_item(&view, "复制值", &key, v.clone()))
                                            .item(copy_item(&view, format!("复制「{k}: …」整行"), &key, format!("{k}: {v}")))
                                            .item(copy_item(&view, "复制名称", &key, k.clone()))
                                    })
                            },
                        )),
                    )
                    .into_any_element()
            }
            RespTab::Tests => {
                if s.tests.is_empty() {
                    return empty_hint("该用例没有断言 —— AI 生成的用例尤其要补上业务断言", cx).into_any_element();
                }
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .p_3()
                    .gap_2()
                    .child(self.toolbar("点击复制表达式，右键复制失败详情", cx).child(self.copy_button(
                        "copy-tests",
                        "复制结果",
                        |this| {
                            this.shown.as_ref().map(|s| {
                                s.tests
                                    .iter()
                                    .map(|t| {
                                        let mark = if t.pass { "✔" } else { "✘" };
                                        match &t.detail {
                                            Some(d) if !t.pass => format!("{mark} {}\n    ↳ {d}", t.expr),
                                            _ => format!("{mark} {}", t.expr),
                                        }
                                    })
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            })
                        },
                        cx,
                    )))
                    .child(v_flex().id("resp-tests").flex_1().min_h_0().overflow_y_scroll().gap_1().children(
                        s.tests.iter().enumerate().map(|(ix, t)| {
                            let (icon, color) = if t.pass {
                                (IconName::CircleCheck, cx.theme().success)
                            } else {
                                (IconName::CircleX, cx.theme().danger)
                            };
                            let key = format!("test:{ix}");
                            let (expr, detail, view) = (t.expr.clone(), t.detail.clone(), view.clone());
                            v_flex()
                                .id(("resp-test", ix))
                                .group("resp-test")
                                .px_1()
                                .py_1()
                                .gap_1()
                                .rounded(cx.theme().radius)
                                .cursor_pointer()
                                .hover(|d| d.bg(cx.theme().muted))
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .text_xs()
                                        .child(Icon::new(icon).xsmall().text_color(color))
                                        .child(div().flex_1().min_w_0().font_family(mono.clone()).child(expr.clone()))
                                        .child(self.copy_mark(&key, "resp-test", cx)),
                                )
                                .when_some(detail.clone(), |this, d| this.child(div().pl_5().text_xs().text_color(muted).child(d)))
                                .on_click(cx.listener({
                                    let (key, expr) = (key.clone(), expr.clone());
                                    move |this, _, _, cx| this.copy(&key, expr.clone(), cx)
                                }))
                                .context_menu(move |menu, _, _| {
                                    let mut menu = menu.item(copy_item(&view, "复制表达式", &key, expr.clone()));
                                    if let Some(d) = &detail {
                                        menu = menu
                                            .item(copy_item(&view, "复制详情", &key, d.clone()))
                                            .item(copy_item(&view, "复制表达式 + 详情", &key, format!("{expr}\n{d}")));
                                    }
                                    menu
                                })
                        }),
                    ))
                    .into_any_element()
            }
            RespTab::Curl => match &s.curl {
                Some(c) => v_flex()
                    .flex_1()
                    .min_h_0()
                    .p_3()
                    .gap_2()
                    .child(
                        self.toolbar(format!("等价的 {} 命令（secret 已脱敏），可直接粘贴到终端复现", command_name(s.protocol)), cx)
                            .child(self.copy_button("copy-curl", "复制命令", |this| this.shown.as_ref().and_then(|s| s.curl.clone()), cx)),
                    )
                    .child(
                        div()
                            .id("curl-text")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .p_3()
                            .rounded(cx.theme().radius)
                            .bg(cx.theme().muted)
                            .cursor_pointer()
                            .text_xs()
                            .font_family(mono)
                            .child(c.clone())
                            .tooltip(|window, cx| Tooltip::new("点击复制").build(window, cx))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(c) = this.shown.as_ref().and_then(|s| s.curl.clone()) {
                                    this.copy("copy-curl", c, cx);
                                }
                            })),
                    )
                    .into_any_element(),
                None => empty_hint("没有编译出的请求", cx).into_any_element(),
            },
            RespTab::Logs => {
                if s.logs.is_empty() {
                    return empty_hint("脚本没有输出日志（在 pre / post 中调用 log(...)）", cx).into_any_element();
                }
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .p_3()
                    .gap_2()
                    .child(
                        self.toolbar(format!("{} 行", s.logs.len()), cx)
                            .child(self.copy_button("copy-logs", "复制全部", |this| this.shown.as_ref().map(|s| s.logs.join("\n")), cx)),
                    )
                    .child(
                        v_flex()
                            .id("resp-logs")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .gap_1()
                            .text_xs()
                            .font_family(mono)
                            .children(s.logs.iter().map(|l| div().child(l.clone()))),
                    )
                    .into_any_element()
            }
        }
    }
}

/// 右键菜单项：复制 text，并在 key 对应的位置显示「已复制」
fn copy_item(view: &WeakEntity<ResponsePanel>, label: impl Into<SharedString>, key: &str, text: String) -> PopupMenuItem {
    let (view, key) = (view.clone(), key.to_string());
    PopupMenuItem::new(label).icon(IconName::Copy).on_click(move |_, _, cx| {
        let _ = view.update(cx, |p, cx| p.copy(&key, text.clone(), cx));
    })
}

/// 等价命令的工具名
fn command_name(p: Protocol) -> &'static str {
    match p {
        Protocol::Http => "cURL",
        Protocol::Grpc => "grpcurl",
        Protocol::Ws => "websocat",
        Protocol::Web | Protocol::Android => "命令",
    }
}

impl ResponsePanel {
    /// 用系统默认程序打开证据文件（截图 / 页面 HTML / logcat…）
    fn open_file(path: std::path::PathBuf, cx: &mut Context<Self>) {
        cx.open_url(&format!("file://{}", path.display()));
    }

    /// 大图查看器：全部截图在一个接近窗口大小的弹窗里，左侧步骤列表，右侧大图（可放大到原始大小）
    fn open_viewer(&mut self, step_ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ev) = self.shown.as_ref().and_then(|s| s.evidence.clone()) else { return };
        let dir = ev.abs_dir(self.root.as_deref());
        let steps: Vec<ViewerStep> = ev
            .steps
            .iter()
            .filter_map(|s| {
                let file = dir.join(s.screenshot.as_ref()?);
                Some(ViewerStep { index: s.index, title: s.title.clone(), status: s.status, detail: s.detail.clone(), at_ms: s.at_ms, duration_ms: s.duration_ms, size: requestbaby_core::engine::ui::image_size(&file), file })
            })
            .collect();
        if steps.is_empty() {
            return;
        }
        let start = ev.steps.get(step_ix).and_then(|s| steps.iter().position(|v| v.index == s.index)).unwrap_or(0);
        let vp = window.viewport_size();
        let (w, h) = (px((f32::from(vp.width) - 80.).max(900.)), px((f32::from(vp.height) - 190.).max(420.)));
        let viewer = cx.new(|cx| StepViewer::new(steps, start, (w, h), window, cx));
        let focus = viewer.clone();
        window.open_dialog(cx, move |dialog, _, _| dialog.title("步骤截图").w(w + px(48.)).child(div().h(h).child(viewer.clone())));
        window.defer(cx, move |window, cx| {
            let handle = focus.read(cx).focus_handle(cx);
            handle.focus(window, cx);
        });
    }

    /// 步骤：每一步一行（状态 · 描述 · 耗时 · 说明），左侧截图缩略图，点击放大；上方运行环境，下方失败现场
    fn render_steps(&self, s: &Shown, cx: &mut Context<Self>) -> impl IntoElement {
        use requestbaby_core::engine::ui::UiStepStatus;
        let Some(ev) = s.evidence.clone() else { return empty_hint("没有步骤记录", cx).into_any_element() };
        let theme = cx.theme();
        let (muted, success, danger, warning, border, radius) = (theme.muted_foreground, theme.success, theme.danger, theme.warning, theme.border, theme.radius);
        let mono = theme.mono_font_family.clone();
        let root = self.root.clone();
        let dir = ev.abs_dir(root.as_deref());
        let shots = ev.steps.iter().filter(|st| st.screenshot.is_some()).count();
        let env_line = ev.environment.iter().filter(|(k, _)| k != "User-Agent").map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(" · ");
        let rows = ev.steps.iter().enumerate().map(|(ix, st)| {
            let (mark, color) = match st.status {
                UiStepStatus::Passed => ("✔", success),
                UiStepStatus::Failed => ("✘", danger),
                UiStepStatus::Skipped => ("⇣", warning),
            };
            let thumb = match &st.screenshot {
                Some(f) => div()
                    .id(("step-shot", ix))
                    .w(px(168.))
                    .h(px(105.))
                    .flex_shrink_0()
                    .rounded(radius)
                    .border_1()
                    .border_color(if st.status == UiStepStatus::Failed { danger } else { border })
                    .overflow_hidden()
                    .cursor_pointer()
                    .child({ use gpui_kit::StyledImage as _; gpui_kit::img(dir.join(f)).w(px(166.)).h(px(103.)).object_fit(gpui_kit::ObjectFit::Contain) })
                    .tooltip(|window, cx| Tooltip::new("点击查看大图").build(window, cx))
                    .on_click(cx.listener(move |this, _, window, cx| this.open_viewer(ix, window, cx)))
                    .into_any_element(),
                None => div().w(px(168.)).h(px(24.)).flex_shrink_0().text_xs().text_color(muted).child("（无截图）").into_any_element(),
            };
            h_flex()
                .id(("ui-step-row", ix))
                .gap_3()
                .py_2()
                .items_start()
                .border_b_1()
                .border_color(border)
                .child(thumb)
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(div().text_color(color).child(mark))
                                .child(div().text_xs().font_family(mono.clone()).text_color(muted).child(format!("#{}", st.index)))
                                .child(div().text_sm().min_w_0().child(st.title.clone())),
                        )
                        .child(div().text_xs().text_color(muted).child(format!("{:.1}s · {}ms", st.at_ms as f64 / 1000.0, st.duration_ms)))
                        .when_some(st.detail.clone(), |this, d| this.child(div().text_xs().text_color(if st.status == UiStepStatus::Failed { danger } else { muted }).child(d))),
                )
        });
        let open_dir = dir.clone();
        let artifacts = ev.artifacts.clone();
        v_flex()
            .flex_1()
            .min_h_0()
            .p_3()
            .gap_2()
            .child(
                self.toolbar(format!("{} 步 · {shots} 张截图 · 点击缩略图查看大图", ev.steps.len()), cx)
                    .when(shots > 0, |t| {
                        let at = ev.failed_step().filter(|s| s.screenshot.is_some()).and_then(|f| ev.steps.iter().position(|s| s.index == f.index)).or_else(|| ev.steps.iter().position(|s| s.screenshot.is_some())).unwrap_or(0);
                        t.child(Button::new("open-viewer").xsmall().outline().icon(IconName::Eye).label("大图浏览").tooltip("在大窗口里逐张查看全部截图，可放大到原始大小").on_click(cx.listener(move |this, _, window, cx| this.open_viewer(at, window, cx))))
                    })
                    .child(Button::new("open-evidence").xsmall().ghost().icon(IconName::FolderOpen).label("打开证据目录").tooltip(ev.dir.clone()).on_click(cx.listener(move |_, _, _, cx| Self::open_file(open_dir.clone(), cx)))),
            )
            .when(!env_line.is_empty(), |this| this.child(div().text_xs().text_color(muted).child(env_line)))
            .child(
                v_flex()
                    .id("ui-steps")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows)
                    .when(!artifacts.is_empty(), |this| {
                        this.child(
                            v_flex()
                                .pt_2()
                                .gap_1()
                                .child(div().text_xs().font_weight(FontWeight::MEDIUM).child("失败现场"))
                                .children(artifacts.into_iter().enumerate().map(|(i, a)| {
                                    let path = dir.join(&a.file);
                                    h_flex()
                                        .gap_2()
                                        .child(div().text_xs().w(px(180.)).child(a.label.clone()))
                                        .child(
                                            Button::new(("artifact", i)).xsmall().ghost().label(a.file.clone()).tooltip("用系统默认程序打开").on_click(cx.listener(move |_, _, _, cx| Self::open_file(path.clone(), cx))),
                                        )
                                })),
                        )
                    }),
            )
            .into_any_element()
    }
}

impl ResponsePanel {
    /// 对话：按时间列出发出（→）与收到（←）的每条消息；点击复制内容
    fn render_dialog(&self, s: &Shown, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, primary, success, border) = (theme.muted_foreground, theme.primary, theme.success, theme.border);
        let mono = theme.mono_font_family.clone();
        let sent = s.frames.iter().filter(|f| f.sent && f.kind != "close").count();
        let received = s.frames.iter().filter(|f| !f.sent && f.kind != "close").count();
        v_flex()
            .flex_1()
            .min_h_0()
            .p_3()
            .gap_2()
            .child(self.toolbar(format!("发出 {sent} 条 · 收到 {received} 条 · 点击一条复制内容"), cx).child(self.copy_button(
                "copy-dialog",
                "复制全部",
                |this| {
                    this.shown.as_ref().map(|s| {
                        s.frames.iter().map(|f| format!("{} {:>6}ms {}{}", if f.sent { "→" } else { "←" }, f.at_ms, if f.kind == "text" || f.kind == "message" { String::new() } else { format!("[{}] ", f.kind) }, f.data)).collect::<Vec<_>>().join("\n")
                    })
                },
                cx,
            )))
            .child(v_flex().id("resp-dialog").flex_1().min_h_0().overflow_y_scroll().children(s.frames.iter().enumerate().map(|(ix, f)| {
                let key = format!("frame:{ix}");
                let data = f.data.clone();
                let (arrow, color) = if f.sent { ("→", primary) } else { ("←", success) };
                let kind = match f.kind.as_str() {
                    "close" => Some("关闭"),
                    "binary" => Some("二进制 base64"),
                    _ => None,
                };
                h_flex()
                    .id(("resp-frame", ix))
                    .group("resp-frame")
                    .gap_3()
                    .px_1()
                    .py_1()
                    .items_start()
                    .text_xs()
                    .border_b_1()
                    .border_color(border)
                    .cursor_pointer()
                    .hover(|d| d.bg(cx.theme().muted))
                    .child(div().w(px(14.)).flex_shrink_0().text_color(color).font_weight(FontWeight::SEMIBOLD).child(arrow))
                    .child(div().w(px(56.)).flex_shrink_0().text_color(muted).font_family(mono.clone()).child(format!("{} ms", f.at_ms)))
                    .when_some(kind, |this, k| this.child(Tag::secondary().xsmall().child(k)))
                    .child(div().flex_1().min_w_0().font_family(mono.clone()).when(f.kind == "close", |d| d.text_color(muted)).child(f.data.clone()))
                    .child(self.copy_mark(&key, "resp-frame", cx))
                    .on_click(cx.listener(move |this, _, _, cx| this.copy(&key, data.clone(), cx)))
            })))
    }
}

fn human_size(n: u64) -> String {
    if n >= 1024 * 1024 {
        format!("{:.1} MB", n as f64 / 1024.0 / 1024.0)
    } else if n >= 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

fn empty_hint(text: impl Into<SharedString>, cx: &mut Context<ResponsePanel>) -> impl IntoElement {
    div()
        .flex_1()
        .p_4()
        .flex()
        .items_center()
        .justify_center()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

impl Render for ResponsePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(s) = self.shown.clone() else {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .text_color(cx.theme().muted_foreground)
                .child(Icon::new(IconName::Inbox).large())
                .child(div().text_sm().child("还没有响应"))
                .child(div().text_xs().child("点击「发送」，或在底部结果中选择一次运行"))
                .into_any_element();
        };
        let tabs = self.visible_tabs();
        let selected = tabs.iter().position(|t| *t == self.tab).unwrap_or(0);
        let labels: Vec<String> = tabs.iter().map(|t| self.tab_label(*t)).collect();
        v_flex()
            .size_full()
            .child(self.render_summary(&s, cx))
            .child(
                h_flex().px_3().pt_2().child(
                    TabBar::new("resp-tabs")
                        .segmented()
                        .small()
                        .selected_index(selected)
                        .children(labels.into_iter().map(|l| Tab::new().label(l)))
                        .on_click(cx.listener(|this, ix: &usize, _, cx| {
                            if let Some(t) = this.visible_tabs().get(*ix) {
                                this.tab = *t;
                            }
                            cx.notify();
                        })),
                ),
            )
            .child(self.render_tab(&s, cx))
            .into_any_element()
    }
}

/// 另存为的默认目录：~/Downloads（不存在则用户主目录）
pub(crate) fn downloads_dir() -> std::path::PathBuf {
    dirs_download()
}

fn dirs_download() -> std::path::PathBuf {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_else(|| ".".into());
    let dl = home.join("Downloads");
    if dl.is_dir() { dl } else { home }
}

#[cfg(test)]
mod tests {
    use super::path_and_query;

    #[test]
    fn path_and_query_strips_scheme_and_host() {
        assert_eq!(path_and_query("http://127.0.0.1:18087/api/v1/items?id=11&page=0"), "/api/v1/items?id=11&page=0");
        assert_eq!(path_and_query("https://api.example.com"), "/");
        assert_eq!(path_and_query("/already/relative?x=1"), "/already/relative?x=1");
    }
}
