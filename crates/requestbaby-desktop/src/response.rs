//! 响应面板：状态 / 响应体 / 响应头 / 断言 / cURL / 脚本日志。
//!
//! 数据来自两处：Desktop 本次运行的完整结果（含完整响应体），
//! 或共享历史中的协议结果（失败用例带 4KB 响应快照）——AI 在 CLI 里跑挂的用例同样能看到现场。
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, h_flex,
    clipboard::Clipboard,
    input::{Editor, EditorState},
    tab::{Tab, TabBar},
    tag::Tag,
    v_flex,
};
use gpui_kit::{
    AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};
use requestbaby_core::report::{CaseReport, TestReport};
use requestbaby_core::runner::CaseResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RespTab {
    Body,
    Headers,
    Tests,
    Curl,
    Logs,
}

impl RespTab {
    const ALL: [RespTab; 5] = [RespTab::Body, RespTab::Headers, RespTab::Tests, RespTab::Curl, RespTab::Logs];
}

/// 面板展示所需的统一视图（来自完整结果或历史协议结果）
#[derive(Debug, Clone, Default)]
struct Shown {
    method: String,
    url: String,
    status: u16,
    duration_ms: u64,
    size: Option<u64>,
    skipped: bool,
    passed: bool,
    error: Option<String>,
    headers: Vec<(String, String)>,
    body: Option<String>,
    body_truncated: bool,
    tests: Vec<TestReport>,
    curl: Option<String>,
    logs: Vec<String>,
    /// 来源说明：本次发送 / 历史运行 id
    origin: String,
}

pub struct ResponsePanel {
    shown: Option<Shown>,
    tab: RespTab,
    body: Entity<EditorState>,
}

fn pretty(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| body.to_string())
}

impl ResponsePanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let body = cx.new(|cx| EditorState::new(window, cx).language("json".to_string()).line_number(true).soft_wrap(true));
        Self { shown: None, tab: RespTab::Body, body }
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
            method: r.method.clone(),
            url: r.url.clone(),
            status: r.status,
            duration_ms: r.duration_ms,
            size: resp.map(|x| x.size),
            skipped: r.skipped,
            passed: r.passed(),
            error: r.error.clone(),
            headers: resp.map(|x| x.headers.clone()).unwrap_or_default(),
            body: resp.map(|x| x.body.clone()),
            body_truncated: false,
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
            method: r.method.clone(),
            url: r.url.clone(),
            status: r.status,
            duration_ms: r.duration_ms,
            size: None,
            skipped: r.skipped,
            passed: r.passed,
            error: r.error.clone(),
            headers: snap.map(|s| s.headers.clone()).unwrap_or_default(),
            body: snap.map(|s| s.body.clone()),
            body_truncated: snap.is_some_and(|s| s.body.chars().count() >= 4000),
            tests: r.tests.clone(),
            curl: r.curl.clone(),
            logs: r.logs.clone(),
            origin: origin.to_string(),
        };
        self.set(shown, window, cx);
    }

    fn set(&mut self, shown: Shown, window: &mut Window, cx: &mut Context<Self>) {
        let text = shown.body.as_deref().map(pretty).unwrap_or_default();
        self.body.update(cx, |s, cx| s.set_value(text, window, cx));
        // 失败时直接把视线引到断言
        let failed_assertions = !shown.passed && !shown.skipped && !shown.tests.is_empty() && shown.error.is_none();
        if failed_assertions || (shown.body.is_none() && !shown.tests.is_empty()) {
            self.tab = RespTab::Tests;
        } else if self.tab == RespTab::Tests && shown.tests.is_empty() {
            self.tab = RespTab::Body;
        }
        self.shown = Some(shown);
        cx.notify();
    }

    fn tab_label(&self, tab: RespTab) -> String {
        let s = self.shown.as_ref();
        match tab {
            RespTab::Body => "响应体".into(),
            RespTab::Headers => match s.map(|s| s.headers.len()).filter(|n| *n > 0) {
                Some(n) => format!("响应头 {n}"),
                None => "响应头".into(),
            },
            RespTab::Tests => match s.filter(|s| !s.tests.is_empty()) {
                Some(s) => format!("断言 {}/{}", s.tests.iter().filter(|t| t.pass).count(), s.tests.len()),
                None => "断言".into(),
            },
            RespTab::Curl => "cURL".into(),
            RespTab::Logs => match s.map(|s| s.logs.len()).filter(|n| *n > 0) {
                Some(n) => format!("日志 {n}"),
                None => "日志".into(),
            },
        }
    }

    fn render_summary(&self, s: &Shown, cx: &mut Context<Self>) -> impl IntoElement {
        let status_tag = if s.skipped {
            Tag::secondary().xsmall().child("SKIPPED")
        } else if s.status == 0 {
            Tag::danger().xsmall().child("无响应")
        } else if s.status >= 400 {
            Tag::danger().xsmall().child(s.status.to_string())
        } else if s.status >= 300 {
            Tag::warning().xsmall().child(s.status.to_string())
        } else {
            Tag::success().xsmall().child(s.status.to_string())
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
            .child(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_color(cx.theme().primary).child(s.method.clone()))
                    .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().text_color(cx.theme().muted_foreground).child(s.url.clone())),
            )
            .when_some(s.error.clone(), |this, e| {
                this.child(
                    h_flex()
                        .mt_1()
                        .gap_2()
                        .items_start()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(Icon::new(IconName::TriangleAlert).xsmall())
                        .child(div().flex_1().child(e)),
                )
            })
    }

    fn render_tab(&self, s: &Shown, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        match self.tab {
            RespTab::Body => match &s.body {
                Some(_) => v_flex()
                    .flex_1()
                    .min_h_0()
                    .p_3()
                    .gap_2()
                    .when(s.body_truncated, |this| {
                        this.child(div().text_xs().text_color(muted).child("历史快照仅保留前 4000 字符；点击「发送」获取完整响应"))
                    })
                    .child(div().flex_1().min_h_0().child(Editor::new(&self.body).readonly(true).h_full()))
                    .into_any_element(),
                None => empty_hint(
                    if s.passed { "历史记录只为失败用例保存响应快照；点击「发送」查看完整响应" } else { "没有响应体" },
                    cx,
                )
                .into_any_element(),
            },
            RespTab::Headers => {
                if s.headers.is_empty() {
                    return empty_hint("没有响应头", cx).into_any_element();
                }
                v_flex()
                    .id("resp-headers")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .children(s.headers.iter().enumerate().map(|(ix, (k, v))| {
                        h_flex()
                            .id(("resp-header", ix))
                            .gap_3()
                            .py_1()
                            .items_start()
                            .text_xs()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .child(div().w(px(180.)).flex_shrink_0().font_weight(FontWeight::MEDIUM).child(k.clone()))
                            .child(div().flex_1().min_w_0().text_color(muted).child(v.clone()))
                    }))
                    .into_any_element()
            }
            RespTab::Tests => {
                if s.tests.is_empty() {
                    return empty_hint("该用例没有断言 —— AI 生成的用例尤其要补上业务断言", cx).into_any_element();
                }
                v_flex()
                    .id("resp-tests")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .gap_1()
                    .children(s.tests.iter().enumerate().map(|(ix, t)| {
                        let (icon, color) = if t.pass {
                            (IconName::CircleCheck, cx.theme().success)
                        } else {
                            (IconName::CircleX, cx.theme().danger)
                        };
                        v_flex()
                            .id(("resp-test", ix))
                            .py_1()
                            .gap_1()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .text_xs()
                                    .child(Icon::new(icon).xsmall().text_color(color))
                                    .child(div().font_family(cx.theme().mono_font_family.clone()).child(t.expr.clone())),
                            )
                            .when_some(t.detail.clone(), |this, d| {
                                this.child(div().pl_5().text_xs().text_color(muted).child(d))
                            })
                    }))
                    .into_any_element()
            }
            RespTab::Curl => match &s.curl {
                Some(c) => v_flex()
                    .flex_1()
                    .min_h_0()
                    .p_3()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .text_xs()
                            .text_color(muted)
                            .child("等价命令（secret 已脱敏），可直接粘贴到终端复现")
                            .child(div().flex_1())
                            .child(Clipboard::new("copy-curl").value(SharedString::from(c.clone()))),
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
                            .text_xs()
                            .font_family(cx.theme().mono_font_family.clone())
                            .child(c.clone()),
                    )
                    .into_any_element(),
                None => empty_hint("没有编译出的请求", cx).into_any_element(),
            },
            RespTab::Logs => {
                if s.logs.is_empty() {
                    return empty_hint("脚本没有输出日志（在 pre / post 中调用 log(...)）", cx).into_any_element();
                }
                v_flex()
                    .id("resp-logs")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .gap_1()
                    .text_xs()
                    .font_family(cx.theme().mono_font_family.clone())
                    .children(s.logs.iter().map(|l| div().child(l.clone())))
                    .into_any_element()
            }
        }
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

fn empty_hint(text: &'static str, cx: &mut Context<ResponsePanel>) -> impl IntoElement {
    div()
        .flex_1()
        .p_4()
        .flex()
        .items_center()
        .justify_center()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text)
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
        let selected = RespTab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
        let labels: Vec<String> = RespTab::ALL.iter().map(|t| self.tab_label(*t)).collect();
        v_flex()
            .size_full()
            .child(self.render_summary(&s, cx))
            .child(
                div().px_3().pt_2().child(
                    TabBar::new("resp-tabs")
                        .underline()
                        .small()
                        .selected_index(selected)
                        .children(labels.into_iter().map(|l| Tab::new().label(l)))
                        .on_click(cx.listener(|this, ix: &usize, _, cx| {
                            this.tab = RespTab::ALL[*ix];
                            cx.notify();
                        })),
                ),
            )
            .child(self.render_tab(&s, cx))
            .into_any_element()
    }
}
