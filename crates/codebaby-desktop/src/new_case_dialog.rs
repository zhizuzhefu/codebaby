//! 新建用例对话框：选协议（HTTP / HTTPS、gRPC、WebSocket、Web、Android、Mac 桌面）→ 填名称与地址；HTTP 也可直接粘贴 cURL。
//!
//! 与 `codebaby new case <目录> <名称> [--method / --url | --curl | --grpc | --ws | --web | --android | --desktop]` 同一实现（core::scaffold）。
//! 底部实时预览将要创建的文件；回车即创建。
use crate::style;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, IndexPath, Selectable as _, Sizable as _, WindowExt as _, h_flex,
    button::{Button, ButtonGroup, ButtonVariants as _},
    input::{Editor, EditorState, Input, InputEvent, InputState},
    select::{Select, SelectState},
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use codebaby_core::scaffold;
use std::path::PathBuf;
use std::rc::Rc;

const METHODS: [&str; 7] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewCaseMode {
    Http,
    /// HTTP，请求从 cURL 命令导入
    Curl,
    Grpc,
    Ws,
    Web,
    Android,
    Desktop,
}

type OnCreated = Rc<dyn Fn(PathBuf, &mut Window, &mut App)>;

pub struct NewCaseForm {
    dir: PathBuf,
    dir_rel: String,
    /// 当前环境的 baseUrl：cURL 里以它开头的 URL 改写为 {{baseUrl}}
    base_url: Option<String>,
    mode: NewCaseMode,
    name: Entity<InputState>,
    method: Entity<SelectState<Vec<SharedString>>>,
    url: Entity<InputState>,
    grpc_target: Entity<InputState>,
    ws_url: Entity<InputState>,
    web_url: Entity<InputState>,
    app: Entity<InputState>,
    mac_app: Entity<InputState>,
    curl: Entity<EditorState>,
    /// 剪贴板里有 curl 命令：HTTP 模式下提示一键使用
    clipboard_curl: Option<String>,
    error: Option<String>,
    on_created: OnCreated,
    _subs: Vec<Subscription>,
}

/// 打开对话框。`dir_rel` 是相对项目根的集合目录
pub fn open(
    dir: PathBuf,
    dir_rel: String,
    base_url: Option<String>,
    mode: NewCaseMode,
    window: &mut Window,
    cx: &mut App,
    on_created: impl Fn(PathBuf, &mut Window, &mut App) + 'static,
) {
    let form = cx.new(|cx| NewCaseForm::new(dir, dir_rel, base_url, mode, Rc::new(on_created), window, cx));
    let focus = form.read(cx).initial_focus(cx);
    window.open_dialog(cx, move |dialog, _, _| dialog.title("新建用例").w(px(600.)).child(form.clone()));
    window.defer(cx, move |window, cx| focus.focus(window, cx));
}

impl NewCaseForm {
    #[allow(clippy::too_many_arguments)]
    fn new(
        dir: PathBuf,
        dir_rel: String,
        base_url: Option<String>,
        mode: NewCaseMode,
        on_created: OnCreated,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let clipboard_curl = cx.read_from_clipboard().and_then(|c| c.text()).filter(|t| t.trim_start().starts_with("curl ")).map(|t| t.trim().to_string());
        let name = cx.new(|cx| InputState::new(window, cx));
        let methods: Vec<SharedString> = METHODS.iter().map(|m| SharedString::from(*m)).collect();
        let method = cx.new(|cx| SelectState::new(methods, Some(IndexPath::new(0)), window, cx));
        let url = cx.new(|cx| InputState::new(window, cx).placeholder("{{baseUrl}}/orders").default_value("{{baseUrl}}/"));
        let grpc_target = cx.new(|cx| InputState::new(window, cx).placeholder("{{grpcTarget}}").default_value("{{grpcTarget}}"));
        let ws_url = cx.new(|cx| InputState::new(window, cx).placeholder("{{wsBase}}/chat").default_value("{{wsBase}}/"));
        let web_url = cx.new(|cx| InputState::new(window, cx).placeholder("{{webBase}}/login").default_value("{{webBase}}/"));
        let app = cx.new(|cx| InputState::new(window, cx).placeholder("com.example.shop"));
        let mac_app = cx.new(|cx| InputState::new(window, cx).placeholder("TextEdit 或 com.apple.TextEdit"));
        let curl = cx.new(|cx| {
            EditorState::new(window, cx)
                .soft_wrap(true)
                .placeholder("curl 'https://api.example.com/v1/orders' -H 'Authorization: Bearer …' --data-raw '{…}'".to_string())
        });
        if mode == NewCaseMode::Curl {
            if let Some(c) = &clipboard_curl {
                curl.update(cx, |s, cx| s.set_value(c.clone(), window, cx));
            }
        }
        // 单行输入回车即创建；名称变化时刷新底部的文件预览
        let mut subs = vec![];
        for input in [&name, &url, &grpc_target, &ws_url, &web_url, &app, &mac_app] {
            subs.push(cx.subscribe_in(input, window, |this: &mut Self, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. } => this.submit(window, cx),
                InputEvent::Change => {
                    this.error = None;
                    cx.notify();
                }
                _ => {}
            }));
        }
        let mut this = Self {
            dir,
            dir_rel,
            base_url,
            mode,
            name,
            method,
            url,
            grpc_target,
            ws_url,
            web_url,
            app,
            mac_app,
            curl,
            clipboard_curl,
            error: None,
            on_created,
            _subs: subs,
        };
        this.sync_name_placeholder(window, cx);
        this
    }

    fn initial_focus(&self, cx: &App) -> FocusHandle {
        match self.mode {
            NewCaseMode::Curl if self.curl.read(cx).value().trim().is_empty() => self.curl.read(cx).focus_handle(cx),
            _ => self.name.read(cx).focus_handle(cx),
        }
    }

    fn set_mode(&mut self, mode: NewCaseMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        self.error = None;
        if mode == NewCaseMode::Curl && self.curl.read(cx).value().trim().is_empty() {
            if let Some(c) = self.clipboard_curl.clone() {
                self.curl.update(cx, |s, cx| s.set_value(c, window, cx));
            }
        }
        self.sync_name_placeholder(window, cx);
        cx.notify();
    }

    fn sync_name_placeholder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let p = if self.mode == NewCaseMode::Curl { "可选：不填则按「方法 + 路径末段」命名" } else { "例如：创建订单" };
        self.name.update(cx, |s, cx| s.set_placeholder(p, window, cx));
    }

    fn value(input: &Entity<InputState>, cx: &App) -> String {
        input.read(cx).value().trim().to_string()
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = Self::value(&self.name, cx);
        if name.is_empty() && self.mode != NewCaseMode::Curl {
            self.error = Some("请填写用例名".into());
            cx.notify();
            return;
        }
        let dir = &self.dir;
        let created = match self.mode {
            NewCaseMode::Http => {
                let method = self.method.read(cx).selected_value().map(|m| m.to_string()).unwrap_or_else(|| "GET".into());
                scaffold::new_case(dir, &name, &method, &Self::value(&self.url, cx))
            }
            NewCaseMode::Curl => {
                let cmd = self.curl.read(cx).value().trim().to_string();
                if cmd.is_empty() {
                    self.error = Some("请粘贴 cURL 命令（浏览器 DevTools 里「Copy as cURL」）".into());
                    cx.notify();
                    return;
                }
                scaffold::new_case_from_curl(dir, Some(name.as_str()).filter(|n| !n.is_empty()), &cmd, self.base_url.as_deref(), None)
            }
            NewCaseMode::Grpc => scaffold::new_grpc_case(dir, &name, &Self::value(&self.grpc_target, cx), "", None),
            NewCaseMode::Ws => scaffold::new_ws_case(dir, &name, &Self::value(&self.ws_url, cx)),
            NewCaseMode::Web => scaffold::new_web_case(dir, &name, &Self::value(&self.web_url, cx)),
            NewCaseMode::Android => scaffold::new_android_case(dir, &name, &Self::value(&self.app, cx)),
            NewCaseMode::Desktop => scaffold::new_desktop_case(dir, &name, &Self::value(&self.mac_app, cx)),
        };
        match created {
            Ok(path) => {
                window.close_dialog(cx);
                (self.on_created)(path, window, cx);
            }
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                cx.notify();
            }
        }
    }

    /// 底部预览：将要创建的文件
    fn preview(&self, cx: &App) -> String {
        let name = Self::value(&self.name, cx);
        let dir = self.dir_rel.trim_end_matches('/');
        if !name.is_empty() {
            return format!("{dir}/{}", scaffold::case_file_name(&self.dir, &name));
        }
        // 还没填名称：显示编号与命名规则
        let prefix = scaffold::case_file_name(&self.dir, "x").trim_end_matches("x.json").to_string();
        match self.mode {
            NewCaseMode::Curl => format!("{dir}/{prefix}<方法-路径末段>.json"),
            _ => format!("{dir}/{prefix}<名称>.json"),
        }
    }

    fn render_protocols(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let current = match self.mode {
            NewCaseMode::Http | NewCaseMode::Curl => 0,
            NewCaseMode::Grpc => 1,
            NewCaseMode::Ws => 2,
            NewCaseMode::Web => 3,
            NewCaseMode::Android => 4,
            NewCaseMode::Desktop => 5,
        };
        ButtonGroup::new("new-case-protocol")
            .outline()
            .child(Button::new("nc-http").label("HTTP / HTTPS").selected(current == 0))
            .child(Button::new("nc-grpc").label("gRPC").selected(current == 1))
            .child(Button::new("nc-ws").label("WebSocket").selected(current == 2))
            .child(Button::new("nc-web").label("Web").selected(current == 3))
            .child(Button::new("nc-android").label("Android").selected(current == 4))
            .child(Button::new("nc-desktop").label("Mac 桌面").selected(current == 5))
            .on_click(cx.listener(|this, clicks: &Vec<usize>, window, cx| {
                let mode = match clicks.first() {
                    Some(1) => NewCaseMode::Grpc,
                    Some(2) => NewCaseMode::Ws,
                    Some(3) => NewCaseMode::Web,
                    Some(4) => NewCaseMode::Android,
                    Some(5) => NewCaseMode::Desktop,
                    _ if this.mode == NewCaseMode::Curl => NewCaseMode::Curl,
                    _ => NewCaseMode::Http,
                };
                this.set_mode(mode, window, cx);
            }))
    }

    fn render_address(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.mode {
            NewCaseMode::Http => {
                let switch_label = if self.clipboard_curl.is_some() { "使用剪贴板里的 cURL" } else { "粘贴 cURL" };
                field_with_action(
                    "请求",
                    Button::new("nc-to-curl")
                        .ghost()
                        .xsmall()
                        .icon(IconName::SquareTerminal)
                        .label(switch_label)
                        .on_click(cx.listener(|this, _, window, cx| this.set_mode(NewCaseMode::Curl, window, cx)))
                        .into_any_element(),
                    h_flex().gap_2().child(div().w(px(120.)).flex_shrink_0().child(Select::new(&self.method))).child(div().flex_1().min_w_0().child(Input::new(&self.url))),
                    Some(https_hint(self.base_url.as_deref())),
                    cx,
                )
            }
            NewCaseMode::Curl => field_with_action(
                "cURL 命令",
                Button::new("nc-to-http")
                    .ghost()
                    .xsmall()
                    .label("改为手动填写")
                    .on_click(cx.listener(|this, _, window, cx| this.set_mode(NewCaseMode::Http, window, cx)))
                    .into_any_element(),
                div().child(Editor::new(&self.curl).h(px(140.))),
                Some(match &self.base_url {
                    Some(b) => format!("支持 -H / -d / --data-urlencode / -F 文件 / -b / -u / -G；以 {b} 开头的 URL 自动改写为 {{{{baseUrl}}}}"),
                    None => "支持 -H / -d / --data-urlencode / -F 文件 / -b / -u / -G".to_string(),
                }),
                cx,
            ),
            NewCaseMode::Grpc => field_with_action(
                "服务地址",
                div().into_any_element(),
                Input::new(&self.grpc_target),
                Some("host:port 为明文，grpcs://host:port 为 TLS。创建后在编辑器顶部从服务的方法列表里选方法，请求消息自动生成".into()),
                cx,
            ),
            NewCaseMode::Ws => field_with_action(
                "地址",
                div().into_any_element(),
                Input::new(&self.ws_url),
                Some("ws:// 或 wss://。创建后在「对话」页逐步添加 发送 / 接收 / 等到".into()),
                cx,
            ),
            NewCaseMode::Web => field_with_action(
                "页面地址",
                div().into_any_element(),
                Input::new(&self.web_url),
                Some("用本机 Chrome / Edge 打开。创建后在「步骤」里添加 点击 / 填写 / 断言，每一步自动截图留证；「读取元素」可直接选按钮和输入框".into()),
                cx,
            ),
            NewCaseMode::Android => field_with_action(
                "应用包名",
                div().into_any_element(),
                Input::new(&self.app),
                Some("通过 adb 在已连接的手机 / 模拟器上启动。创建后在「步骤」里添加 点按 / 填写 / 断言，每一步自动截图留证".into()),
                cx,
            ),
            NewCaseMode::Desktop => field_with_action(
                "应用名称或 bundle ID",
                div().into_any_element(),
                Input::new(&self.mac_app),
                Some("通过 agent-desktop 操作 Mac 应用（需先 npm install -g agent-desktop，并给运行 codebaby 的程序开启辅助功能权限）。创建后在「步骤」里添加 点击 / 填写 / 断言，每一步自动截图留证".into()),
                cx,
            ),
        }
        .into_any_element()
    }
}

/// 一个表单字段：标题（右侧可放一个操作）+ 控件 + 可选说明
fn field_with_action(label: &'static str, action: AnyElement, control: impl IntoElement, hint: Option<String>, cx: &App) -> gpui_kit::Div {
    v_flex()
        .gap_1p5()
        .child(h_flex().justify_between().child(div().text_sm().font_weight(FontWeight::MEDIUM).child(label)).child(action))
        .child(control)
        .when_some(hint, |this, h| this.child(div().text_xs().text_color(cx.theme().muted_foreground).child(h)))
}

impl Focusable for NewCaseForm {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.name.read(cx).focus_handle(cx)
    }
}

impl Render for NewCaseForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, danger, border) = (theme.muted_foreground, theme.danger, theme.border);
        let mono = theme.mono_font_family.clone();
        let preview = self.preview(cx);
        v_flex()
            .gap_4()
            .child(
                h_flex()
                    .gap_1p5()
                    .text_xs()
                    .text_color(muted)
                    .child(Icon::new(IconName::Folder).xsmall())
                    .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(format!("保存到 {}/", self.dir_rel.trim_end_matches('/')))),
            )
            .child(self.render_protocols(cx))
            // cURL 模式命令是主角、名称可选：命令放前面
            .map(|this| {
                let name = field_with_action("名称", div().into_any_element(), Input::new(&self.name), None, cx);
                if self.mode == NewCaseMode::Curl {
                    this.child(self.render_address(cx)).child(name)
                } else {
                    this.child(name).child(self.render_address(cx))
                }
            })
            .when_some(self.error.clone(), |this, e| {
                this.child(
                    h_flex()
                        .gap_2()
                        .items_start()
                        .text_xs()
                        .text_color(danger)
                        .child(Icon::new(IconName::TriangleAlert).xsmall())
                        .child(div().flex_1().min_w_0().child(e)),
                )
            })
            .child(
                h_flex()
                    .pt_3()
                    .gap_2()
                    .border_t_1()
                    .border_color(border)
                    .child(Icon::new(IconName::FileText).xsmall().text_color(muted))
                    .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().text_xs().font_family(mono).text_color(muted).child(preview))
                    .child(Button::new("nc-cancel").outline().label("取消").on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(
                        Button::new("nc-create")
                            .primary()
                            .rounded(style::PILL)
                            .icon(IconName::Plus)
                            .label("创建")
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}

/// HTTP 与 HTTPS 是同一种用例：按地址的协议头区分，说清楚当前环境用的是哪个
fn https_hint(base_url: Option<&str>) -> String {
    let now = match base_url {
        Some(b) if b.starts_with("https://") => format!("当前环境的 baseUrl 是 {b}，即 HTTPS。"),
        Some(b) if b.starts_with("http://") => format!("当前环境的 baseUrl 是 {b}，即 HTTP。"),
        _ => String::new(),
    };
    format!("{now}地址以 https:// 开头就是 HTTPS，证书按系统信任校验；测试环境的自签证书可在用例里打开「跳过 TLS 证书校验」")
}
