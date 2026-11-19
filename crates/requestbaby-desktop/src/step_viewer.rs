//! 步骤截图查看器：接近整个窗口大小，左侧是全部步骤（缩略图 + 标题，几十步也能一眼定位并点选），
//! 右侧是大图；可「适应窗口」「原始大小」（1 个图片像素 = 1 个逻辑像素，与浏览器 100% 一致）和任意缩放，← → 翻页。
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Selectable as _, Sizable as _, h_flex,
    button::{Button, ButtonVariants as _},
    v_flex,
};
use gpui_kit::{
    Context, FocusHandle, Focusable, FontWeight, InteractiveElement as _, IntoElement, KeyDownEvent, ObjectFit, ParentElement as _, Pixels,
    Render, ScrollHandle, StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window, div, img, prelude::FluentBuilder as _, px,
};
use requestbaby_core::engine::ui::UiStepStatus;
use std::path::PathBuf;

/// 侧栏宽度
const LIST_W: f32 = 300.;
/// 标题 / 提示占用的高度（用来估算「适应窗口」的缩放比例）
const CHROME_H: f32 = 96.;

pub struct ViewerStep {
    pub index: usize,
    pub title: String,
    pub status: UiStepStatus,
    pub detail: Option<String>,
    pub at_ms: u64,
    pub duration_ms: u64,
    pub file: PathBuf,
    /// 图片像素尺寸
    pub size: Option<(u32, u32)>,
}

pub struct StepViewer {
    steps: Vec<ViewerStep>,
    cur: usize,
    /// None = 适应窗口；Some(k) = 原始大小的 k 倍
    zoom: Option<f32>,
    /// 弹窗内容区大小（估算适应窗口的比例）
    size: (f32, f32),
    focus: FocusHandle,
    list: ScrollHandle,
}

impl StepViewer {
    pub fn new(steps: Vec<ViewerStep>, start: usize, size: (Pixels, Pixels), cx: &mut Context<Self>) -> Self {
        let this = Self {
            steps,
            cur: start,
            zoom: None,
            size: (f32::from(size.0), f32::from(size.1)),
            focus: cx.focus_handle(),
            list: ScrollHandle::new(),
        };
        this.list.scroll_to_item(start);
        this
    }

    fn go(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.steps.len() {
            self.cur = ix;
            self.list.scroll_to_item(ix);
            cx.notify();
        }
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = self.cur as isize + delta;
        if n >= 0 {
            self.go(n as usize, cx);
        }
    }

    /// 原始大小下的显示尺寸（逻辑像素）：1 个图片像素 = 1 个逻辑像素，和浏览器里图片 100% 一致
    /// （截图是 1 倍图；按物理像素一一对应的话，Retina 屏上「原始大小」反而比「适应窗口」更小）
    fn real_size(&self) -> Option<(f32, f32)> {
        let (w, h) = self.steps[self.cur].size?;
        Some((w as f32, h as f32))
    }

    /// 适应窗口时的实际缩放比例（相对原始大小）
    fn fit_scale(&self) -> f32 {
        let Some((w, h)) = self.real_size() else { return 1. };
        let (aw, ah) = (self.size.0 - LIST_W - 24., self.size.1 - CHROME_H);
        (aw / w).min(ah / h).clamp(0.05, 8.)
    }

    fn set_zoom(&mut self, zoom: Option<f32>, cx: &mut Context<Self>) {
        self.zoom = zoom.map(|k| k.clamp(0.1, 8.));
        cx.notify();
    }

    fn zoom_by(&mut self, factor: f32, cx: &mut Context<Self>) {
        let now = self.zoom.unwrap_or_else(|| self.fit_scale());
        self.set_zoom(Some(now * factor), cx);
    }

    fn on_key(&mut self, key: &str, cx: &mut Context<Self>) {
        match key {
            "left" | "pageup" | "k" => self.step(-1, cx),
            "right" | "pagedown" | "j" => self.step(1, cx),
            "home" => self.go(0, cx),
            "end" => self.go(self.steps.len() - 1, cx),
            "+" | "=" => self.zoom_by(1.25, cx),
            "-" => self.zoom_by(0.8, cx),
            "0" => self.set_zoom(None, cx),
            "1" => self.set_zoom(Some(1.), cx),
            _ => {}
        }
    }

    fn mark(status: UiStepStatus, cx: &Context<Self>) -> (&'static str, gpui_kit::Hsla) {
        let t = cx.theme();
        match status {
            UiStepStatus::Passed => ("✔", t.success),
            UiStepStatus::Failed => ("✘", t.danger),
            UiStepStatus::Skipped => ("⇣", t.warning),
        }
    }
}

impl Focusable for StepViewer {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for StepViewer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (muted, border, radius, danger) = (cx.theme().muted_foreground, cx.theme().border, cx.theme().radius, cx.theme().danger);
        let accent = cx.theme().primary;
        let st = &self.steps[self.cur];
        let (mark, mark_color) = Self::mark(st.status, cx);
        let last = self.steps.len() - 1;
        let first_failed = self.steps.iter().position(|s| s.status == UiStepStatus::Failed);
        let zoom_label = match self.zoom {
            None => format!("适应窗口 {:.0}%", self.fit_scale() * 100.),
            Some(k) => format!("{:.0}%", k * 100.),
        };

        let rows = self.steps.iter().enumerate().map(|(ix, s)| {
            let (m, c) = Self::mark(s.status, cx);
            let cur = ix == self.cur;
            h_flex()
                .id(("viewer-step", ix))
                .gap_2()
                .p_1()
                .items_center()
                .rounded(radius)
                .cursor_pointer()
                .border_1()
                .border_color(if cur { accent } else { gpui_kit::transparent_black() })
                .when(cur, |d| d.bg(cx.theme().table_active))
                .hover(|d| d.bg(cx.theme().table_hover))
                .on_click(cx.listener(move |this, _, _, cx| this.go(ix, cx)))
                .child(div().w(px(84.)).h(px(53.)).flex_none().rounded(px(3.)).overflow_hidden().border_1().border_color(if s.status == UiStepStatus::Failed { danger } else { border }).child(img(s.file.clone()).w(px(82.)).h(px(51.)).object_fit(ObjectFit::Cover)))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(h_flex().gap_1().text_xs().child(div().text_color(c).child(m)).child(div().text_color(muted).child(format!("#{}", s.index))))
                        .child(div().text_xs().line_clamp(2).child(s.title.clone())),
                )
        });

        let stage = div().id("viewer-stage").flex_1().min_h_0().rounded(radius).border_1().border_color(border).cursor_pointer();
        let stage = match (self.zoom, self.real_size()) {
            (Some(k), Some((w, h))) => stage
                .overflow_scroll()
                .child(img(st.file.clone()).w(px(w * k)).h(px(h * k)).flex_none().object_fit(ObjectFit::Fill)),
            _ => stage.overflow_hidden().child(img(st.file.clone()).size_full().object_fit(ObjectFit::Contain)),
        }
        .on_click(cx.listener(|this, _, _, cx| {
            let z = if this.zoom.is_none() { Some(1.) } else { None };
            this.set_zoom(z, cx);
        }));

        h_flex()
            .size_full()
            .gap_3()
            .track_focus(&self.focus)
            .key_context("StepViewer")
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.on_key(ev.keystroke.key.as_str(), cx)))
            .child(
                v_flex()
                    .w(px(LIST_W))
                    .h_full()
                    .flex_none()
                    .gap_1()
                    .child(div().text_xs().text_color(muted).child(format!("共 {} 张截图 · 点击切换", self.steps.len())))
                    .child(v_flex().id("viewer-list").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.list).gap_1().children(rows)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().text_color(mark_color).child(mark))
                            .child(div().flex_1().min_w_0().text_sm().font_weight(FontWeight::MEDIUM).overflow_hidden().text_ellipsis().whitespace_nowrap().child(format!("#{} {}", st.index, st.title)))
                            .child(div().text_xs().text_color(muted).child(format!("{:.1}s · {}ms", st.at_ms as f64 / 1000., st.duration_ms)))
                            .child(div().text_xs().text_color(muted).child(format!("{} / {}", self.cur + 1, self.steps.len())))
                            .child(Button::new("v-prev").xsmall().ghost().icon(IconName::ChevronLeft).tooltip("上一张（←）").disabled(self.cur == 0).on_click(cx.listener(|this, _, _, cx| this.step(-1, cx))))
                            .child(Button::new("v-next").xsmall().ghost().icon(IconName::ChevronRight).tooltip("下一张（→）").disabled(self.cur == last).on_click(cx.listener(|this, _, _, cx| this.step(1, cx))))
                            .when_some(first_failed, |d, f| d.child(Button::new("v-failed").xsmall().outline().label("失败的一步").tooltip("跳到第一个失败的步骤").on_click(cx.listener(move |this, _, _, cx| this.go(f, cx)))))
                            .child(Button::new("v-fit").xsmall().ghost().label("适应").tooltip("适应窗口（0）").selected(self.zoom.is_none()).on_click(cx.listener(|this, _, _, cx| this.set_zoom(None, cx))))
                            .child(Button::new("v-real").xsmall().ghost().label("原始大小").tooltip("原始大小：1 个图片像素 = 1 个逻辑像素，与浏览器 100% 一致（1）").selected(self.zoom == Some(1.)).on_click(cx.listener(|this, _, _, cx| this.set_zoom(Some(1.), cx))))
                            .child(Button::new("v-out").xsmall().ghost().icon(IconName::Minus).tooltip("缩小（-）").on_click(cx.listener(|this, _, _, cx| this.zoom_by(0.8, cx))))
                            .child(div().w(px(84.)).text_xs().text_color(muted).text_center().child(zoom_label))
                            .child(Button::new("v-in").xsmall().ghost().icon(IconName::Plus).tooltip("放大（+）").on_click(cx.listener(|this, _, _, cx| this.zoom_by(1.25, cx)))),
                    )
                    .when_some(st.detail.clone(), |d, t| d.child(div().text_xs().text_color(if st.status == UiStepStatus::Failed { danger } else { muted }).child(t)))
                    .child(div().text_xs().text_color(muted).child("← → 切换步骤 · 点击图片或按 1 / 0 在原始大小与适应窗口间切换 · 放大后用触控板或滚轮平移 · Esc 关闭"))
                    .child(stage),
            )
    }
}
