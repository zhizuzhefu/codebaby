//! 用例编辑器里 gRPC / WebSocket 的界面（与 HTTP 同一个编辑器、同一套断言 / 脚本 / 高级页，只有请求部分不同）：
//! - gRPC：地址 + 方法（从服务端反射或 .proto 加载方法列表）· 消息（按方法生成骨架）· 元数据
//! - WebSocket：地址 · 对话（一行一步：发送 / 接收 / 等到 / 静候 / 关闭）· 请求头 · 参数
//!
//! Web / Android / Mac 桌面见 case_editor_ui.rs
use super::{CaseEditor, EditorTab};
use crate::case_form::StepForm;
use crate::kv_table::ReadonlyRow;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, h_flex,
    button::{Button, ButtonVariants as _},
    input::{Editor, Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    switch::Switch,
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Styled as _, Window, div, prelude::FluentBuilder as _, px,
};
use requestbaby_core::config::{Protocol, WsAction};
use requestbaby_core::engine::grpc::MethodInfo;
use serde_json::Value;

/// 对话里的一行
pub(super) struct StepRow {
    pub action: WsAction,
    pub content: Entity<InputState>,
    pub capture: Entity<InputState>,
    pub timeout: Entity<InputState>,
    /// 打开时的原样步骤（没改就原样写回）
    pub raw: Option<Value>,
}

fn content_placeholder(a: WsAction) -> &'static str {
    match a {
        WsAction::Send => "要发送的消息：{\"type\": \"join\", \"room\": 1} 或 ping；支持 {{变量}}，数字写 \"{{id:number}}\"",
        WsAction::SendBinary => "二进制内容（base64），如 AQID",
        WsAction::Receive => "下一条消息必须满足：{\"type\": \"joined\"}（写法同断言；{} = 任意消息；文本消息写 {\"$\": \"pong\"}）",
        WsAction::WaitFor => "一直等到满足条件的消息（其他消息跳过）：{\"type\": \"message\", \"data.from\": \"bob\"}",
        WsAction::Wait => "静候毫秒数，期间收到的消息照常记录",
        WsAction::Close => "关闭码（留空 = 1000 正常关闭）",
    }
}

/// gRPC 元数据里不会发送的 HTTP 专用头（与 runner 一致）
const GRPC_RESERVED: [&str; 8] = ["content-type", "content-length", "user-agent", "host", "connection", "te", "accept", "accept-encoding"];

impl CaseEditor {
    /// 界面用例地址栏的提示：Web 写页面地址，Android 写包名，Mac 桌面写应用名称
    pub(super) fn sync_ui_target_placeholder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let hint = match self.protocol {
            Protocol::Android => "com.example.shop（应用包名）",
            Protocol::Desktop => "TextEdit 或 com.apple.TextEdit（应用名称 / bundle ID）",
            _ => "{{webBase}}/login（要打开的页面）",
        };
        self.ui_target.update(cx, |st, cx| st.set_placeholder(hint, window, cx));
    }

    /// 切换协议：请求部分换成新协议的界面；状态码是旧协议的缺省值时一并换成新协议的缺省值
    pub(super) fn set_protocol(&mut self, p: Protocol, window: &mut Window, cx: &mut Context<Self>) {
        if p == self.protocol {
            return;
        }
        let old_default = default_status(self.protocol);
        self.protocol = p;
        let status = self.expect_status.read(cx).value().to_string();
        if status.trim() == old_default {
            let new = default_status(p).to_string();
            self.expect_status.update(cx, |st, cx| st.set_value(new, window, cx));
        }
        let fill = |input: &Entity<InputState>, value: &str, window: &mut Window, cx: &mut Context<Self>| {
            if input.read(cx).value().trim().is_empty() {
                let v = value.to_string();
                input.update(cx, |st, cx| st.set_value(v, window, cx));
            }
        };
        match p {
            Protocol::Http => fill(&self.url, "{{baseUrl}}/", window, cx),
            Protocol::Grpc => {
                fill(&self.grpc_target, "{{grpcTarget}}", window, cx);
                if self.grpc_methods.is_none() {
                    self.load_grpc_methods(cx);
                }
            }
            Protocol::Ws => fill(&self.ws_url, "{{wsBase}}/", window, cx),
            Protocol::Web => fill(&self.ui_target, "{{webBase}}/", window, cx),
            Protocol::Android | Protocol::Desktop => {
                // 地址 ↔ 包名 / 应用名不通用：从 Web 切过来时清空
                if self.ui_target.read(cx).value().contains("://") || self.ui_target.read(cx).value().contains("{{") {
                    self.ui_target.update(cx, |st, cx| st.set_value(String::new(), window, cx));
                }
            }
        }
        self.ui_snapshot = None;
        self.sync_ui_target_placeholder(window, cx);
        if self.tab == EditorTab::Json {
            self.tab = EditorTab::Request;
        }
        self.sync_proto_hints(cx);
        self.refresh_dirty(cx);
        cx.notify();
    }

    // ---------------------------------------------------------------- WebSocket 步骤

    fn step_row(&mut self, f: &StepForm, window: &mut Window, cx: &mut Context<Self>) -> StepRow {
        let content = cx.new(|cx| InputState::new(window, cx).placeholder(content_placeholder(f.action)).default_value(f.content.clone()));
        let capture = cx.new(|cx| InputState::new(window, cx).placeholder("捕获：sid=data.sid").default_value(f.capture.clone()));
        let timeout = cx.new(|cx| InputState::new(window, cx).placeholder("超时 ms").default_value(f.timeout.clone()));
        for input in [&content, &capture, &timeout] {
            self._step_subs.push(cx.subscribe_in(input, window, |this, _, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.refresh_dirty(cx);
                }
            }));
        }
        StepRow { action: f.action, content, capture, timeout, raw: f.raw.clone() }
    }

    pub(super) fn set_steps(&mut self, steps: &[StepForm], window: &mut Window, cx: &mut Context<Self>) {
        self._step_subs.clear();
        self.ws_steps = steps.iter().map(|f| self.step_row(f, window, cx)).collect();
    }

    fn current_steps(&self, cx: &gpui_kit::App) -> Vec<StepForm> {
        let input = |e: &Entity<InputState>| e.read(cx).value().to_string();
        self.ws_steps
            .iter()
            .map(|r| StepForm { action: r.action, content: input(&r.content), capture: input(&r.capture), timeout: input(&r.timeout), raw: r.raw.clone() })
            .collect()
    }

    fn edit_steps(&mut self, f: impl FnOnce(&mut Vec<StepForm>), window: &mut Window, cx: &mut Context<Self>) {
        let mut steps = self.current_steps(cx);
        f(&mut steps);
        self.set_steps(&steps, window, cx);
        self.refresh_dirty(cx);
        cx.notify();
    }

    fn add_step(&mut self, a: WsAction, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_steps(|s| s.push(StepForm::new(a)), window, cx);
    }

    fn set_step_action(&mut self, ix: usize, a: WsAction, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_steps(
            |s| {
                if let Some(st) = s.get_mut(ix) {
                    if st.action != a {
                        // 内容在两种动作间通用（发送 ↔ 发送、接收 ↔ 等到）时保留，否则换成新动作的缺省内容
                        let keep = matches!((st.action, a), (WsAction::Receive, WsAction::WaitFor) | (WsAction::WaitFor, WsAction::Receive));
                        let mut n = StepForm::new(a);
                        if keep {
                            n.content = st.content.clone();
                            n.capture = st.capture.clone();
                        }
                        n.timeout = st.timeout.clone();
                        *st = n;
                    }
                }
            },
            window,
            cx,
        );
    }

    fn move_step(&mut self, ix: usize, up: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_steps(
            |s| {
                let to = if up { ix.checked_sub(1) } else { Some(ix + 1).filter(|t| *t < s.len()) };
                if let Some(to) = to {
                    s.swap(ix, to);
                }
            },
            window,
            cx,
        );
    }

    fn remove_step(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_steps(
            |s| {
                if ix < s.len() {
                    s.remove(ix);
                }
            },
            window,
            cx,
        );
    }

    // ---------------------------------------------------------------- gRPC 方法

    /// 后台加载方法列表（项目配置了 .proto 就读文件，否则向服务端反射）；与 `requestbaby grpc list` 同一实现
    pub(super) fn load_grpc_methods(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.path.as_deref().and_then(requestbaby_core::runner::discover::find_project_root) else { return };
        let env = if self.env.is_empty() { "dev".to_string() } else { self.env.clone() };
        let target = self.grpc_target.read(cx).value().to_string();
        let insecure = self.insecure;
        self.grpc_loading = true;
        let task = cx.background_spawn(async move { requestbaby_core::protocols::grpc_methods(&root, &env, &target, insecure).map_err(|e| format!("{e:#}")) });
        self._grpc_task = Some(cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |e, cx| {
                e.grpc_loading = false;
                e.grpc_methods = Some(res);
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn method_info(&self, cx: &gpui_kit::App) -> Option<MethodInfo> {
        let m = self.grpc_method.read(cx).value().trim().to_string();
        let list = self.grpc_methods.as_ref()?.as_ref().ok()?;
        list.iter().find(|i| i.method == m || i.method.replace('/', ".") == m).cloned()
    }

    /// 选中方法：写入方法名，按方法类型设置单条 / 多条消息；消息还是空的就填骨架
    fn pick_method(&mut self, info: MethodInfo, window: &mut Window, cx: &mut Context<Self>) {
        let m = info.method.clone();
        self.grpc_method.update(cx, |st, cx| st.set_value(m, window, cx));
        self.grpc_streaming = info.client_streaming;
        let current = self.grpc_message.read(cx).value().trim().to_string();
        if current.is_empty() || current == "{}" || current == "[]" || current == "[{}]" {
            self.fill_template(&info, window, cx);
        }
        self.refresh_dirty(cx);
        cx.notify();
    }

    fn fill_template(&mut self, info: &MethodInfo, window: &mut Window, cx: &mut Context<Self>) {
        let v = if self.grpc_streaming { Value::Array(vec![info.template.clone()]) } else { info.template.clone() };
        let text = serde_json::to_string_pretty(&v).unwrap_or_default();
        self.grpc_message.update(cx, |st, cx| st.set_value(text, window, cx));
        self.refresh_dirty(cx);
    }

    fn beautify_grpc_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.grpc_message.read(cx).value().to_string();
        match serde_json::from_str::<Value>(&text) {
            Ok(v) => {
                let pretty = serde_json::to_string_pretty(&v).unwrap_or(text);
                self.grpc_message.update(cx, |s, cx| s.set_value(pretty, window, cx));
            }
            Err(e) => {
                self.error = Some(format!("不是合法 JSON，无法格式化：{e}"));
                cx.notify();
            }
        }
    }

    fn set_grpc_streaming(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.grpc_streaming = on;
        // 单条 ↔ 多条：包一层 / 拆一层，内容不丢
        let text = self.grpc_message.read(cx).value().to_string();
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            let next = match (on, v) {
                (true, Value::Array(a)) => Value::Array(a),
                (true, one) => Value::Array(vec![one]),
                (false, Value::Array(a)) if a.len() == 1 => a.into_iter().next().unwrap(),
                (false, other) => other,
            };
            let pretty = serde_json::to_string_pretty(&next).unwrap_or(text);
            self.grpc_message.update(cx, |s, cx| s.set_value(pretty, window, cx));
        }
        self.refresh_dirty(cx);
        cx.notify();
    }

    /// 元数据 / 握手请求头表格下方的只读行：集合 collection.json 的 headers 也会一起发送
    pub(super) fn sync_proto_hints(&mut self, cx: &mut Context<Self>) {
        let grpc_rows: Vec<ReadonlyRow> = self
            .inherited_headers
            .iter()
            .filter(|r| !GRPC_RESERVED.contains(&r.key.to_lowercase().as_str()))
            .map(|r| ReadonlyRow { key: r.key.to_lowercase(), value: r.value.clone(), source: r.source.clone() })
            .collect();
        let ws_rows: Vec<ReadonlyRow> = self.inherited_headers.iter().filter(|r| !r.key.eq_ignore_ascii_case("content-type")).cloned().collect();
        self.grpc_metadata.update(cx, |t, cx| t.set_readonly(grpc_rows, cx));
        self.ws_headers.update(cx, |t, cx| t.set_readonly(ws_rows, cx));
    }

    // ---------------------------------------------------------------- 渲染

    /// 地址栏右侧「方法」按钮：列出服务的全部方法，点选即填入（缺省选中当前方法）
    pub(super) fn render_method_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let view = cx.entity().downgrade();
        let methods: Vec<MethodInfo> = self.grpc_methods.as_ref().and_then(|r| r.as_ref().ok()).cloned().unwrap_or_default();
        let error = self.grpc_methods.as_ref().and_then(|r| r.as_ref().err()).cloned();
        let current = self.grpc_method.read(cx).value().to_string();
        // 紧凑：只显示方法数，把宽度留给方法名
        let label = if self.grpc_loading {
            "…".to_string()
        } else if error.is_some() {
            "⚠".to_string()
        } else {
            methods.len().to_string()
        };
        let tip = if self.grpc_loading {
            "正在加载方法列表…".to_string()
        } else if let Some(e) = &error {
            format!("方法列表加载失败：{e}")
        } else {
            format!("从服务的 {} 个方法里选择", methods.len())
        };
        h_flex()
            .gap_1()
            .child(
                Button::new("grpc-methods")
                    .small()
                    .outline()
                    .icon(IconName::ChevronDown)
                    .label(label)
                    .tooltip(tip)
                    .disabled(self.grpc_loading)
                    .dropdown_menu(move |menu, _, _| {
                        let mut menu = menu;
                        if let Some(e) = &error {
                            menu = menu.item(PopupMenuItem::new(format!("加载失败：{}", e.chars().take(80).collect::<String>())).disabled(true));
                        } else if methods.is_empty() {
                            menu = menu.item(PopupMenuItem::new("没有方法：检查地址，或在 requestbaby.json 配置 grpc.protos").disabled(true));
                        }
                        for m in &methods {
                            let (v, info) = (view.clone(), m.clone());
                            let text = format!("{}  · {}", m.method, m.kind_label());
                            menu = menu.item(PopupMenuItem::new(text).checked(m.method == current).on_click(move |_, window, cx| {
                                let _ = v.update(cx, |e, cx| e.pick_method(info.clone(), window, cx));
                            }));
                        }
                        menu
                    }),
            )
            .child(
                Button::new("grpc-reload")
                    .small()
                    .ghost()
                    .icon(IconName::RefreshCw)
                    .tooltip("重新加载方法列表（等价命令：requestbaby grpc list）")
                    .disabled(self.grpc_loading)
                    .on_click(cx.listener(|this, _, _, cx| this.load_grpc_methods(cx))),
            )
            .into_any_element()
    }

    pub(super) fn render_grpc_parts(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, success, border, radius) = (theme.muted_foreground, theme.success, theme.border, theme.radius);
        let hint = |t: String| div().text_xs().text_color(muted).child(t);
        let meta_n = self.grpc_metadata.read(cx).active_count(cx);
        let info = self.method_info(cx);
        let tabs = TabBar::new("grpc-parts")
            .underline()
            .small()
            .selected_index(self.grpc_tab)
            .child(Tab::new().label("消息"))
            .child(Tab::new().label("元数据").when(meta_n > 0, |t| t.suffix(div().text_xs().text_color(success).child(format!("{meta_n}")))))
            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                this.grpc_tab = *ix;
                cx.notify();
            }));
        let kind = match &info {
            Some(i) => format!("{} · {} → {}", i.kind_label(), i.input_type, i.output_type),
            None => "从「方法列表」选择方法后显示请求 / 响应类型".into(),
        };
        let body = if self.grpc_tab == 0 {
            let info_t = info.clone();
            v_flex()
                .gap_2()
                .child(
                    h_flex()
                        .gap_3()
                        .child(div().text_xs().font_weight(FontWeight::MEDIUM).child(if self.grpc_streaming { "请求消息（按顺序发送）" } else { "请求消息" }))
                        .child(
                            Switch::new("grpc-streaming")
                                .small()
                                .label("多条消息（客户端流 / 双向流）")
                                .checked(self.grpc_streaming)
                                .on_change(cx.listener(|this, v: &bool, window, cx| this.set_grpc_streaming(*v, window, cx))),
                        )
                        .child(div().flex_1())
                        .child(
                            Button::new("grpc-template")
                                .xsmall()
                                .ghost()
                                .label("生成骨架")
                                .tooltip("按方法的请求类型填入全部字段（等价命令：requestbaby grpc describe <方法>）")
                                .disabled(info.is_none())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if let Some(i) = &info_t {
                                        this.fill_template(i, window, cx);
                                        cx.notify();
                                    }
                                })),
                        )
                        .child(Button::new("grpc-beautify").xsmall().ghost().label("格式化").on_click(cx.listener(|this, _, window, cx| this.beautify_grpc_message(window, cx)))),
                )
                .child(div().h(px(280.)).rounded(radius).border_1().border_color(border).overflow_hidden().child(Editor::new(&self.grpc_message).h_full()))
                .child(hint(kind))
                .child(hint("字段名同 .proto（lowerCamelCase 也接受）；数字变量写 \"{{id:number}}\"；写错字段名会在发送前报出可用字段".into()))
        } else {
            v_flex()
                .gap_2()
                .child(h_flex().gap_2().child(div().text_xs().font_weight(FontWeight::MEDIUM).child("请求元数据")).child(hint("名称小写，相当于 HTTP 请求头；灰色行继承自集合，会一起发送".into())))
                .child(self.grpc_metadata.clone())
        };
        let source_hint = match &self.grpc_methods {
            Some(Err(e)) => Some(div().text_xs().text_color(cx.theme().warning).child(format!("方法列表加载失败：{e}"))),
            _ => None,
        };
        v_flex().gap_2().child(tabs).child(body).children(source_hint).into_any_element()
    }

    pub(super) fn render_ws_parts(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, success) = (theme.muted_foreground, theme.success);
        let hint = |t: &'static str| div().text_xs().text_color(muted).child(t);
        let badge = |n: usize| div().text_xs().text_color(success).child(format!("{n}"));
        let header_n = self.ws_headers.read(cx).active_count(cx);
        let query_n = self.ws_query.read(cx).active_count(cx);
        let steps_n = self.ws_steps.len();
        let tabs = TabBar::new("ws-parts")
            .underline()
            .small()
            .selected_index(self.ws_tab)
            .child(Tab::new().label("对话").when(steps_n > 0, |t| t.suffix(badge(steps_n))))
            .child(Tab::new().label("请求头").when(header_n > 0, |t| t.suffix(badge(header_n))))
            .child(Tab::new().label("参数").when(query_n > 0, |t| t.suffix(badge(query_n))))
            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                this.ws_tab = *ix;
                cx.notify();
            }));
        let body = match self.ws_tab {
            0 => self.render_steps(cx),
            1 => v_flex()
                .gap_2()
                .child(h_flex().gap_2().child(div().text_xs().font_weight(FontWeight::MEDIUM).child("握手请求头")).child(hint("如 Authorization；灰色行继承自集合；同一主机的 Cookie 自动带上")))
                .child(self.ws_headers.clone())
                .into_any_element(),
            _ => v_flex()
                .gap_2()
                .child(h_flex().gap_2().child(div().text_xs().font_weight(FontWeight::MEDIUM).child("查询参数")).child(hint("URL 编码后追加到地址")))
                .child(self.ws_query.clone())
                .into_any_element(),
        };
        v_flex().gap_2().child(tabs).child(body).into_any_element()
    }

    /// 对话：一行一步，按顺序执行；任一步失败即停止
    fn render_steps(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, border, radius) = (theme.muted_foreground, theme.border, theme.radius);
        let mono = theme.mono_font_family.clone();
        let view = cx.entity().downgrade();
        let n = self.ws_steps.len();
        let rows = self.ws_steps.iter().enumerate().map(|(ix, r)| {
            let v = view.clone();
            let current = r.action;
            let is_recv = matches!(r.action, WsAction::Receive | WsAction::WaitFor);
            h_flex()
                .id(("ws-step", ix))
                .gap_2()
                .py_1()
                .border_b_1()
                .border_color(cx.theme().table_row_border)
                .child(div().w(px(22.)).text_xs().font_family(mono.clone()).text_color(muted).child(format!("{}", ix + 1)))
                .child(
                    Button::new(("ws-action", ix))
                        .small()
                        .outline()
                        .w(px(96.))
                        .label(r.action.label())
                        .dropdown_menu(move |menu, _, _| {
                            let mut menu = menu;
                            for a in WsAction::ALL {
                                let v = v.clone();
                                menu = menu.item(PopupMenuItem::new(a.label()).checked(a == current).on_click(move |_, window, cx| {
                                    let _ = v.update(cx, |e, cx| e.set_step_action(ix, a, window, cx));
                                }));
                            }
                            menu
                        }),
                )
                .child(div().flex_1().min_w_0().child(Input::new(&r.content).small()))
                .when(is_recv, |this| this.child(div().w(px(150.)).child(Input::new(&r.capture).small())))
                .when(!matches!(r.action, WsAction::Send | WsAction::SendBinary | WsAction::Close | WsAction::Wait), |this| this.child(div().w(px(70.)).child(Input::new(&r.timeout).small())))
                .child(
                    h_flex()
                        .child(Button::new(("ws-up", ix)).xsmall().ghost().icon(IconName::ArrowUp).disabled(ix == 0).on_click(cx.listener(move |this, _, window, cx| this.move_step(ix, true, window, cx))))
                        .child(Button::new(("ws-down", ix)).xsmall().ghost().icon(IconName::ArrowDown).disabled(ix + 1 == n).on_click(cx.listener(move |this, _, window, cx| this.move_step(ix, false, window, cx))))
                        .child(Button::new(("ws-del", ix)).xsmall().ghost().icon(IconName::Close).on_click(cx.listener(move |this, _, window, cx| this.remove_step(ix, window, cx)))),
                )
        });
        let add = |a: WsAction, cx: &mut Context<Self>| {
            Button::new(("ws-add", a as usize)).xsmall().outline().icon(IconName::Plus).label(a.label()).on_click(cx.listener(move |this, _, window, cx| this.add_step(a, window, cx)))
        };
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(div().text_xs().font_weight(FontWeight::MEDIUM).child("对话步骤"))
                    .child(div().text_xs().text_color(muted).child("连接成功后按顺序执行；接收 / 等到的条件写法同断言，可捕获变量给后面的步骤用；任一步失败即停止")),
            )
            .child(if self.ws_steps.is_empty() {
                div()
                    .h(px(72.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(radius)
                    .border_1()
                    .border_dashed()
                    .border_color(border)
                    .text_sm()
                    .text_color(muted)
                    .child("没有步骤：只验证能否连上（握手）")
                    .into_any_element()
            } else {
                v_flex().children(rows).into_any_element()
            })
            .child(
                h_flex()
                    .gap_1()
                    .flex_wrap()
                    .child(add(WsAction::Send, cx))
                    .child(add(WsAction::Receive, cx))
                    .child(add(WsAction::WaitFor, cx))
                    .child(add(WsAction::Wait, cx))
                    .child(add(WsAction::SendBinary, cx))
                    .child(add(WsAction::Close, cx)),
            )
            .into_any_element()
    }
}

/// 新建 / 切换协议时的缺省期望状态
pub(super) fn default_status(p: Protocol) -> &'static str {
    match p {
        Protocol::Http => "200",
        Protocol::Grpc => "OK",
        Protocol::Ws => "101",
        Protocol::Web => "200",
        Protocol::Android | Protocol::Desktop => "",
    }
}
