//! 通用对话框：单行输入（新建集合 / 重命名）、环境自检结果。新建用例见 new_case_dialog
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex,
    button::{Button, ButtonVariant, ButtonVariants as _},
    input::{Input, InputState},
    notification::Notification,
    v_flex,
};
use gpui_kit::{App, AppContext as _, ClipboardItem, Focusable as _, InteractiveElement as _, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px};
use codebaby_core::doctor::{CheckLevel, DoctorReport};
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
        let text = report
            .checks
            .iter()
            .map(|c| {
                let mark = match c.level {
                    CheckLevel::Ok => "✓",
                    CheckLevel::Warn => "!",
                    CheckLevel::Error => "✗",
                };
                format!("{mark} {}", c.message)
            })
            .collect::<Vec<_>>()
            .join("\n");
        dialog
            .title(if ok { "环境自检通过" } else { "环境自检发现问题" })
            .w(px(680.))
            .child(
                v_flex()
                    .id("doctor-checks")
                    .max_h(px(460.))
                    .overflow_y_scroll()
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
                            // 长路径 / URL 没有空格：min_w_0 让它在对话框内折行，而不是被截断
                            .child(div().flex_1().min_w_0().child(c.message.clone()))
                    }))
                    ,
            )
            .footer(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(crate::style::cli_button("doctor-cli", "codebaby doctor"))
                    .child(div().flex_1())
                    .child(Button::new("doctor-copy").outline().label("复制结果").on_click(move |_, window, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                        window.push_notification("已复制自检结果", cx);
                    }))
                    .child(Button::new("doctor-close").primary().label("好").on_click(|_, window, cx| window.close_dialog(cx))),
            )
    });
}

/// 打开某文档会丢弃未保存修改时的确认框
pub fn confirm_discard(lost: Vec<String>, window: &mut Window, cx: &mut App, then: impl Fn(&mut Window, &mut App) + 'static) {
    let then = std::rc::Rc::new(then);
    window.open_alert_dialog(cx, move |alert, _, _| {
        let then = then.clone();
        alert
            .title(format!("放弃对「{}」的修改？", lost.join("」「")))
            .description("修改尚未写入文件。")
            .confirm()
            .ok_text("放弃修改")
            .ok_variant(ButtonVariant::Danger)
            .cancel_text("继续编辑")
            .on_ok(move |_, window, cx| {
                then(window, cx);
                true
            })
    });
}

/// 危险操作确认（删除等）：确认后执行 `then`
pub fn confirm_danger(
    window: &mut Window,
    cx: &mut App,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    ok_text: &'static str,
    then: impl Fn(&mut Window, &mut App) + 'static,
) {
    let (title, description): (SharedString, SharedString) = (title.into(), description.into());
    let then = std::rc::Rc::new(then);
    window.open_alert_dialog(cx, move |alert, _, _| {
        let then = then.clone();
        alert
            .title(title.clone())
            .description(description.clone())
            .confirm()
            .ok_text(ok_text)
            .ok_variant(ButtonVariant::Danger)
            .cancel_text("取消")
            .on_ok(move |_, window, cx| {
                then(window, cx);
                true
            })
    });
}

/// 「关于」：版本号、定位、链接；复制版本号方便反馈问题时贴上
pub fn show_about(window: &mut Window, cx: &mut App) {
    let version = codebaby_core::update::CURRENT;
    let build = format!("{version} ({} {})", std::env::consts::OS, std::env::consts::ARCH);
    window.open_dialog(cx, move |dialog, _, cx| {
        let muted = cx.theme().muted_foreground;
        let build = build.clone();
        let link = |id: &'static str, label: &'static str, url: &'static str| Button::new(id).ghost().small().icon(IconName::ExternalLink).label(label).on_click(move |_, _, cx| cx.open_url(url));
        dialog
            .title("关于 codebaby")
            .w(px(460.))
            .child(
                v_flex()
                    .gap_2()
                    .items_center()
                    .child(Icon::new(IconName::Bot).size_8().text_color(cx.theme().primary))
                    .child(div().text_xl().font_weight(gpui_kit::FontWeight::SEMIBOLD).child("codebaby"))
                    .child(div().text_sm().child(format!("版本 {version}")))
                    .child(div().text_xs().text_color(muted).child(format!("{} · {}", if cfg!(target_os = "macos") { "macOS" } else { std::env::consts::OS }, std::env::consts::ARCH)))
                    .child(div().pt_2().text_sm().child("AI 交付的独立验收层"))
                    .child(div().text_xs().text_color(muted).child("辅助人对 AI 交付的成果验收 · MIT 许可"))
                    .child(
                        h_flex()
                            .pt_2()
                            .gap_1()
                            .child(link("about-site", "官网与下载", codebaby_core::about::WEBSITE))
                            .child(link("about-docs", "文档", codebaby_core::about::DOCS))
                            .child(link("about-src", "源码", codebaby_core::about::SOURCE))
                            .child(link("about-issues", "反馈", codebaby_core::about::ISSUES)),
                    ),
            )
            .footer(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(Button::new("about-copy").outline().label("复制版本号").on_click(move |_, window, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(format!("codebaby {build}")));
                        window.push_notification(Notification::success("已复制版本号"), cx);
                    }))
                    .child(Button::new("about-update").outline().label("检查更新").on_click(|_, window, cx| {
                        window.close_dialog(cx);
                        window.dispatch_action(Box::new(crate::CheckUpdate), cx);
                    }))
                    .child(Button::new("about-close").primary().label("好").on_click(|_, window, cx| window.close_dialog(cx))),
            )
    });
}
