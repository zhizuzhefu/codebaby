//! 通用对话框：单行输入（新建 / 重命名）与环境自检结果。
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex,
    button::{Button, ButtonVariants as _},
    input::{Input, InputState},
    notification::Notification,
    v_flex,
};
use gpui_kit::{App, AppContext as _, Focusable as _, ParentElement as _, SharedString, Styled as _, Window, div, px};
use requestbaby_core::doctor::{CheckLevel, DoctorReport};
use std::rc::Rc;

/// 单行输入对话框。`on_ok` 返回错误时保留对话框并提示
pub fn prompt_text(
    window: &mut Window,
    cx: &mut App,
    title: impl Into<SharedString>,
    hint: impl Into<SharedString>,
    initial: impl Into<SharedString>,
    ok_label: &'static str,
    on_ok: impl Fn(String, &mut Window, &mut App) -> anyhow::Result<()> + 'static,
) {
    let initial: SharedString = initial.into();
    let input = cx.new(|cx| InputState::new(window, cx).default_value(initial));
    let focus = input.read(cx).focus_handle(cx);
    let (title, hint): (SharedString, SharedString) = (title.into(), hint.into());
    let on_ok = Rc::new(on_ok);
    window.open_dialog(cx, move |dialog, _, cx| {
        let submit = {
            let input = input.clone();
            let on_ok = on_ok.clone();
            move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
                let value = input.read(cx).value().trim().to_string();
                match on_ok(value, window, cx) {
                    Ok(()) => window.close_dialog(cx),
                    Err(e) => window.push_notification(Notification::error(format!("{e:#}")), cx),
                }
            }
        };
        dialog
            .title(title.clone())
            .w(px(460.))
            .child(
                v_flex()
                    .gap_2()
                    .child(Input::new(&input))
                    .child(div().text_xs().text_color(cx.theme().muted_foreground).child(hint.clone())),
            )
            .footer(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(Button::new("prompt-cancel").outline().label("取消").on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(Button::new("prompt-ok").primary().label(ok_label).on_click(submit)),
            )
    });
    window.defer(cx, move |window, cx| focus.focus(window, cx));
}

pub fn show_doctor(report: DoctorReport, window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, move |dialog, _, cx| {
        let ok = report.ok();
        dialog
            .title(if ok { "环境自检通过" } else { "环境自检发现问题" })
            .w(px(560.))
            .child(
                v_flex()
                    .gap_2()
                    .children(report.checks.iter().map(|c| {
                        let (icon, color) = match c.level {
                            CheckLevel::Ok => (IconName::CircleCheck, cx.theme().success),
                            CheckLevel::Warn => (IconName::TriangleAlert, cx.theme().warning),
                            CheckLevel::Error => (IconName::CircleX, cx.theme().danger),
                        };
                        h_flex()
                            .gap_2()
                            .items_start()
                            .text_sm()
                            .child(Icon::new(icon).small().text_color(color))
                            .child(div().flex_1().child(c.message.clone()))
                    }))
                    .child(
                        div()
                            .pt_2()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("等价命令：requestbaby doctor（CLI 与桌面端同一实现）"),
                    ),
            )
            .footer(h_flex().justify_end().child(Button::new("doctor-close").primary().label("好").on_click(|_, window, cx| window.close_dialog(cx))))
    });
}
