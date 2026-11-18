//! 视觉体系：Apple 系统配色（assets/theme.json）+ 画布 / 卡片分层 + HTTP 方法配色。
//!
//! 层次：窗口是浅灰画布（标题栏、侧栏、状态栏直接放在画布上），编辑区 / 响应 / 结果是浮在画布上的圆角卡片。
use gpui_kit::base::{ResizeHandleRenderer, ResizeHandleState};
use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode, ThemeSet};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{IconName, Sizable as _, WindowExt as _};
use gpui_kit::{App, Axis, ClipboardItem, Div, ElementId, Hsla, IntoElement as _, ParentElement as _, SharedString, Styled as _, div, px, rgb};
use std::rc::Rc;

const THEME: &str = include_str!("../assets/theme.json");

/// 卡片圆角
pub const CARD_RADIUS: f32 = 12.;
/// 卡片之间、卡片与窗口边缘的间距
pub const GAP: f32 = 8.;
/// 胶囊形按钮（主操作：发送 / 验证）
pub const PILL: gpui_kit::Pixels = px(999.);

/// 注册并启用 requestbaby 主题；明暗跟随系统（`REQUESTBABY_THEME=dark|light` 可覆盖）
pub fn install(cx: &mut App) {
    let set: ThemeSet = serde_json::from_str(THEME).expect("assets/theme.json 格式错误");
    for config in set.themes {
        let config = Rc::new(config);
        Theme::update(cx, |t| {
            if config.mode.is_dark() {
                t.dark_theme = config.clone();
            } else {
                t.light_theme = config.clone();
            }
        });
    }
    let mode = match std::env::var("REQUESTBABY_THEME").as_deref() {
        Ok("dark") => ThemeMode::Dark,
        Ok("light") => ThemeMode::Light,
        _ => cx.window_appearance().into(),
    };
    Theme::change(mode, None, cx);
}

/// 窗口画布色（标题栏 / 侧栏 / 状态栏所在的底）
pub fn canvas(cx: &App) -> Hsla {
    cx.theme().title_bar
}

/// 卡片：画布上的圆角内容面板
pub fn card(cx: &App) -> Div {
    let dark = cx.theme().mode.is_dark();
    div()
        .bg(cx.theme().background)
        .rounded(px(CARD_RADIUS))
        .border_1()
        .border_color(if dark { gpui_kit::hsla(0., 0., 1., 0.06) } else { gpui_kit::hsla(0., 0., 0., 0.06) })
        .shadow_sm()
        .overflow_hidden()
}

/// HTTP 方法配色（与 Postman 等工具的习惯一致：GET 绿 / POST 橙 / PUT 蓝 / PATCH 紫 / DELETE 红）
pub fn method_color(method: &str, cx: &App) -> Hsla {
    let dark = cx.theme().mode.is_dark();
    let hex = match method.to_ascii_uppercase().as_str() {
        "GET" => if dark { 0x30d158 } else { 0x1f9d48 },
        "POST" => if dark { 0xff9f0a } else { 0xd97400 },
        "PUT" => if dark { 0x409cff } else { 0x0a6fdb },
        "PATCH" => if dark { 0xbf5af2 } else { 0x9a3fd0 },
        "DELETE" => if dark { 0xff6961 } else { 0xe0352b },
        _ => return cx.theme().muted_foreground,
    };
    rgb(hex).into()
}

/// 方法标签：等宽、加粗、按方法着色
pub fn method_label(method: &str, cx: &App) -> Div {
    let short = match method.to_ascii_uppercase().as_str() {
        "DELETE" => "DEL".to_string(),
        "OPTIONS" => "OPT".to_string(),
        m => m.to_string(),
    };
    div()
        .text_xs()
        .font_weight(gpui_kit::FontWeight::BOLD)
        .text_color(method_color(method, cx))
        .child(short)
}

/// 卡片之间的分隔条：静止时透明（只留出间隙），悬停 / 拖动时显示胶囊提示
pub fn gap_handle() -> ResizeHandleRenderer {
    Rc::new(|handle, _, cx| {
        let engaged = !matches!(handle.state(), ResizeHandleState::Idle);
        let horizontal = matches!(handle.axis(), Axis::Horizontal);
        let line = div().flex_none().flex().items_center().justify_center();
        let line = if horizontal { line.w(px(1.)).h_full() } else { line.h(px(1.)).w_full() };
        Some(
            line.when(engaged, |line| {
                let pill = div().flex_none().rounded_full().bg(cx.theme().muted_foreground).opacity(0.5);
                line.child(if horizontal { pill.w(px(4.)).h(px(36.)) } else { pill.h(px(4.)).w(px(36.)) })
            })
            .into_any_element(),
        )
    })
}

/// 等价 CLI 命令按钮：不占版面，悬停看命令、点击复制（每个操作都能在终端 / CI / AI 里原样复现）
pub fn cli_button(id: impl Into<ElementId>, command: impl Into<SharedString>) -> Button {
    let command: SharedString = command.into();
    Button::new(id).ghost().xsmall().icon(IconName::SquareTerminal).tooltip(format!("等价命令（点击复制）\n{command}")).on_click(move |_, window, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string(command.to_string()));
        window.push_notification(Notification::success(format!("已复制：{command}")), cx);
    })
}
