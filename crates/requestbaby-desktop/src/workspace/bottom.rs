//! 底部面板：结果（含工作流步骤与报告导出）/ 日志 / 历史 / 校验，以及状态栏。
use super::{BottomTab, Workspace, relative_time, run_origin};
use crate::project::{CaseStatus, status_of};
use crate::run::{Phase, ResultsSource, StepState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex,
    button::Button,
    chart::BarChart,
    clipboard::Clipboard,
    menu::{DropdownMenu as _, PopupMenuItem},
    progress::Progress,
    status_bar::StatusBar,
    stepper::{Stepper, StepperItem},
    tab::{Tab, TabBar},
    tag::Tag,
    v_flex,
};
use gpui_kit::{
    AnyElement, Context, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    Stateful, StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};
use requestbaby_core::report::{CaseReport, RunReport};
use std::path::PathBuf;

impl Workspace {
    pub(super) fn render_bottom(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let run = self.run.read(cx);
        let rows = run.rows().len();
        let failed = run.rows().iter().filter(|r| !r.passed && !r.skipped).count();
        let logs = run.logs().len();
        let history = self.project.history().len();
        let v = &self.validation;
        let errors = v.issues.iter().filter(|i| i.level == "error").count();
        let warns = v.issues.len() - errors;
        let labels = [
            if failed > 0 { format!("结果 {rows} · {failed} 失败") } else { format!("结果 {rows}") },
            format!("日志 {logs}"),
            format!("历史 {history}"),
            match (v.running && !v.done, errors, warns) {
                (true, _, _) => "校验 …".to_string(),
                (_, 0, 0) => "校验 ✓".to_string(),
                (_, 0, w) => format!("校验 · {w} 警告"),
                (_, e, _) => format!("校验 · {e} 错误"),
            },
        ];
        let selected = BottomTab::ALL.iter().position(|t| *t == self.bottom_tab).unwrap_or(0);
        v_flex()
            .size_full()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                h_flex().px_3().pt_1().child(
                    TabBar::new("bottom-tabs")
                        .underline()
                        .small()
                        .selected_index(selected)
                        .children(labels.into_iter().map(|l| Tab::new().label(l)))
                        .on_click(cx.listener(|this, ix: &usize, _, cx| {
                            this.bottom_tab = BottomTab::ALL[*ix];
                            cx.notify();
                        })),
                ),
            )
            .map(|this| match self.bottom_tab {
                BottomTab::Results => this.child(self.render_results(cx)),
                BottomTab::Logs => this.child(self.render_logs(cx)),
                BottomTab::History => this.child(self.render_history(cx)),
                BottomTab::Validate => this.child(self.render_validate(cx)),
            })
    }

    fn render_results(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let run = self.run.read(cx);
        let rows: Vec<CaseReport> = run.rows().to_vec();
        let running = run.is_streaming();
        let planned = run.planned();
        let passed = rows.iter().filter(|r| r.passed).count();
        let failed = rows.iter().filter(|r| !r.passed && !r.skipped).count();
        let skipped = rows.iter().filter(|r| r.skipped).count();
        let elapsed = run.elapsed_ms();
        let command = run.command().to_string();
        let source = run.source().clone();
        let phase = run.phase().clone();
        let steps = run.steps().to_vec();
        let workflow = run.workflow().map(String::from);
        let has_report = run.report().is_some();
        let dry_run = run.report().and_then(|r| r.meta.as_ref()).is_some_and(|m| m.dry_run);
        let origin = match &source {
            ResultsSource::Live => "本次运行".to_string(),
            ResultsSource::History(id) => self
                .project
                .history()
                .iter()
                .find(|r| r.meta.as_ref().is_some_and(|m| &m.id == id))
                .map(run_origin)
                .unwrap_or_else(|| "历史".into()),
        };
        let muted = cx.theme().muted_foreground;

        if rows.is_empty() && steps.is_empty() && !running {
            let hint = match phase {
                Phase::Failed(e) => e,
                _ => "还没有运行记录。点击「运行全部」、运行工作流，或在终端执行 requestbaby run —— 结果会自动出现在这里".into(),
            };
            return empty_state(IconName::SquareTerminal, hint, cx);
        }

        let header = h_flex()
            .px_3()
            .py_2()
            .gap_3()
            .child(div().text_xs().font_weight(FontWeight::MEDIUM).child(match &workflow {
                Some(w) => format!("{origin} · 工作流 {w}"),
                None => origin,
            }))
            .when(dry_run, |this| this.child(Tag::info().outline().xsmall().child("dry-run")))
            .child(Tag::success().outline().xsmall().child(format!("{passed} 通过")))
            .when(failed > 0, |this| this.child(Tag::danger().outline().xsmall().child(format!("{failed} 失败"))))
            .when(skipped > 0, |this| this.child(Tag::secondary().outline().xsmall().child(format!("{skipped} 跳过"))))
            .child(div().text_xs().text_color(muted).child(format!("{elapsed} ms")))
            .when(running && planned > 0, |this| {
                let pct = (rows.len() as f32 / planned as f32 * 100.).min(100.);
                this.child(div().w(px(160.)).child(Progress::new("run-progress").value(pct)))
            })
            .child(div().flex_1())
            .when(!command.is_empty(), |this| {
                this.child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_xs()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_color(muted)
                        .child(command.clone()),
                )
                .child(Clipboard::new("copy-command").value(SharedString::from(command.clone())).tooltip("复制等价 CLI 命令"))
            })
            .child({
                let view = cx.entity().downgrade();
                Button::new("export-report")
                    .xsmall()
                    .outline()
                    .icon(IconName::ExternalLink)
                    .label("导出")
                    .disabled(!has_report || running)
                    .dropdown_menu(move |menu, _, _| {
                        let (v1, v2) = (view.clone(), view.clone());
                        menu.item(PopupMenuItem::new("HTML 报告").icon(IconName::Globe).on_click(move |_, window, cx| {
                            let _ = v1.update(cx, |w, cx| w.export_report(true, window, cx));
                        }))
                        .item(PopupMenuItem::new("JSON 报告（requestbaby/run@1）").icon(IconName::FileText).on_click(
                            move |_, window, cx| {
                                let _ = v2.update(cx, |w, cx| w.export_report(false, window, cx));
                            },
                        ))
                    })
            });

        let hover = cx.theme().table_hover;
        let active = cx.theme().table_active;
        let selected_row = self.selected_row;
        let head = result_cols(
            h_flex()
                .id("results-head")
                .px_2()
                .py_1()
                .gap_2()
                .text_xs()
                .text_color(cx.theme().table_head_foreground)
                .bg(cx.theme().table_head)
                .border_b_1()
                .border_color(cx.theme().border),
            [
                div().into_any_element(),
                "用例".into_any_element(),
                "文件".into_any_element(),
                "方法".into_any_element(),
                "状态".into_any_element(),
                "耗时".into_any_element(),
                "断言".into_any_element(),
            ],
        );
        let body = rows.iter().enumerate().map(|(ix, r)| {
            let (icon, color) = match status_of(r) {
                CaseStatus::Passed => (IconName::CircleCheck, cx.theme().success),
                CaseStatus::Failed => (IconName::CircleX, cx.theme().danger),
                CaseStatus::Skipped => (IconName::Ban, muted),
            };
            let tests_pass = r.tests.iter().filter(|t| t.pass).count();
            let status = if r.status == 0 { "-".to_string() } else { r.status.to_string() };
            let status_color = if r.status >= 400 || (r.status == 0 && !dry_run) { cx.theme().danger } else { cx.theme().foreground };
            let tests_color = if tests_pass < r.tests.len() { cx.theme().danger } else { cx.theme().foreground };
            let row = h_flex()
                .id(("result-row", ix))
                .px_2()
                .py_1()
                .gap_2()
                .text_xs()
                .border_b_1()
                .border_color(cx.theme().table_row_border)
                .when(selected_row == Some(ix), |this| this.bg(active))
                .hover(|this| this.bg(hover))
                .on_click(cx.listener(move |this, _, window, cx| this.select_row(ix, window, cx)));
            result_cols(
                row,
                [
                    Icon::new(icon).xsmall().text_color(color).into_any_element(),
                    div().font_weight(FontWeight::MEDIUM).child(r.name.clone()).into_any_element(),
                    div().text_color(muted).child(r.file.clone()).into_any_element(),
                    div().text_color(muted).child(r.method.clone()).into_any_element(),
                    div().text_color(status_color).child(status).into_any_element(),
                    div().text_color(muted).child(format!("{} ms", r.duration_ms)).into_any_element(),
                    div()
                        .text_color(tests_color)
                        .child(if r.tests.is_empty() { "-".to_string() } else { format!("{tests_pass}/{}", r.tests.len()) })
                        .into_any_element(),
                ],
            )
        });

        v_flex()
            .flex_1()
            .min_h_0()
            .child(header)
            .when(!steps.is_empty(), |this| {
                // 已结束 / 进行中的最后一步作为 stepper 的进度位置
                let at = steps.iter().rposition(|s| !matches!(s.state, StepState::Pending | StepState::NotRun)).unwrap_or(0);
                let items = steps.iter().map(|s| {
                    let (icon, note) = match s.state {
                        StepState::Pending => (IconName::Dash, "等待".to_string()),
                        StepState::Running => (IconName::LoaderCircle, format!("运行中 · {} 个用例", s.planned)),
                        StepState::Passed => (IconName::CircleCheck, format!("{}/{} 通过", s.passed, s.passed + s.failed)),
                        StepState::Failed => (IconName::CircleX, format!("{} 失败 / {}", s.failed, s.passed + s.failed)),
                        StepState::NotRun => (IconName::Ban, "未执行".to_string()),
                    };
                    let note = if s.label == s.run { note } else { format!("{} · {note}", s.run) };
                    StepperItem::new().icon(icon).child(
                        v_flex()
                            .child(div().text_xs().font_weight(FontWeight::MEDIUM).child(s.label.clone()))
                            .child(div().text_xs().text_color(muted).child(note)),
                    )
                });
                this.child(div().px_4().pb_2().child(Stepper::new("workflow-steps").small().items(items).selected_index(at)))
            })
            .child(div().px_2().child(head))
            .child(div().id("results-scroll").flex_1().min_h_0().overflow_y_scroll().px_2().children(body))
            .into_any_element()
    }

    fn select_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (row, source) = {
            let run = self.run.read(cx);
            (run.rows().get(ix).cloned(), run.source().clone())
        };
        let Some(row) = row else { return };
        self.selected_row = Some(ix);
        if self.project.case(&row.file).is_some() {
            self.open(super::Target::Case(row.file.clone()), window, cx);
        }
        match source {
            ResultsSource::Live => {
                if let Some(r) = self.run.read(cx).response(&row.file).cloned() {
                    self.response.update(cx, |p, cx| p.show_result(&r, "本次运行", window, cx));
                } else {
                    self.response.update(cx, |p, cx| p.show_report(&row, "本次运行", window, cx));
                }
            }
            ResultsSource::History(id) => {
                let origin = self
                    .project
                    .history()
                    .iter()
                    .find(|r| r.meta.as_ref().is_some_and(|m| m.id == id))
                    .map(run_origin)
                    .unwrap_or_default();
                self.response.update(cx, |p, cx| p.show_report(&row, &origin, window, cx));
            }
        }
        cx.notify();
    }

    fn render_logs(&self, cx: &mut Context<Self>) -> AnyElement {
        let logs = self.run.read(cx).logs().to_vec();
        if logs.is_empty() {
            return empty_state(IconName::FileText, "脚本日志会实时出现在这里（pre / post / setup 中的 log(...)）".into(), cx);
        }
        v_flex()
            .id("logs-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_4()
            .py_2()
            .gap_1()
            .text_xs()
            .font_family(cx.theme().mono_font_family.clone())
            .children(logs.into_iter().enumerate().map(|(ix, l)| {
                h_flex()
                    .id(("log", ix))
                    .gap_3()
                    .child(div().w(px(280.)).flex_shrink_0().text_color(cx.theme().muted_foreground).child(l.file))
                    .child(div().when(l.notice, |d| d.text_color(cx.theme().warning)).child(l.text))
            }))
            .into_any_element()
    }

    fn render_history(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let runs: Vec<RunReport> = self.project.history().to_vec();
        let muted = cx.theme().muted_foreground;
        if runs.is_empty() {
            let dir = requestbaby_core::history::runs_dir(self.project.root());
            return empty_state(IconName::Inbox, format!("暂无运行历史 —— CLI 与 Desktop 共享 {}", dir.display()), cx);
        }
        #[derive(Clone)]
        struct Point {
            label: String,
            rate: f64,
            failed: bool,
        }
        let points: Vec<Point> = runs
            .iter()
            .take(24)
            .rev()
            .enumerate()
            .map(|(ix, r)| Point {
                label: format!("#{}", ix + 1),
                rate: if r.total == 0 { 0. } else { r.passed as f64 / r.total as f64 * 100. },
                failed: r.failed > 0,
            })
            .collect();
        let (ok, bad) = (cx.theme().success, cx.theme().danger);
        let chart = BarChart::new(points)
            .band(|p: &Point| p.label.clone())
            .value(|p: &Point| p.rate)
            .fill(move |p: &Point, _, _, _| if p.failed { bad } else { ok })
            .value_axis(true)
            .value_tick_count(3)
            .id("history-pass-rate");

        let current = match self.run.read(cx).source() {
            ResultsSource::History(id) => Some(id.clone()),
            _ => None,
        };
        let (hover, active) = (cx.theme().table_hover, cx.theme().table_active);
        h_flex()
            .flex_1()
            .min_h_0()
            .items_stretch()
            .child(
                v_flex()
                    .w(px(360.))
                    .flex_shrink_0()
                    .p_3()
                    .gap_2()
                    .border_r_1()
                    .border_color(cx.theme().border)
                    .child(div().text_xs().font_weight(FontWeight::MEDIUM).child("通过率趋势（最近 24 次）"))
                    .child(div().flex_1().min_h_0().child(chart)),
            )
            .child(
                v_flex()
                    .id("history-list")
                    .flex_1()
                    .min_w_0()
                    .overflow_y_scroll()
                    .p_2()
                    .children(runs.into_iter().enumerate().map(|(ix, r)| {
                        let m = r.meta.clone().unwrap_or_default();
                        let is_current = current.as_deref() == Some(m.id.as_str());
                        let (icon, color) =
                            if r.failed > 0 { (IconName::CircleX, cx.theme().danger) } else { (IconName::CircleCheck, cx.theme().success) };
                        let report = r.clone();
                        h_flex()
                            .id(("history-row", ix))
                            .px_2()
                            .py_1()
                            .gap_3()
                            .rounded(cx.theme().radius)
                            .when(is_current, |this| this.bg(active))
                            .hover(|this| this.bg(hover))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.run.update(cx, |run, cx| run.show_report(&report, cx));
                                this.selected_row = None;
                                this.bottom_tab = BottomTab::Results;
                                cx.notify();
                            }))
                            .child(Icon::new(icon).xsmall().text_color(color))
                            .child(div().w(px(64.)).child(Tag::secondary().xsmall().child(m.source.clone())))
                            .child(div().w(px(84.)).text_xs().child(format!("{}/{} 通过", r.passed, r.total)))
                            .child(div().w(px(56.)).text_xs().text_color(muted).child(m.env.clone()))
                            .child(div().w(px(80.)).text_xs().text_color(muted).child(relative_time(m.started_at_ms)))
                            .when_some(m.workflow.clone(), |this, w| this.child(Tag::info().outline().xsmall().child(format!("工作流 {w}"))))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .text_xs()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_color(muted)
                                    .child(m.command.clone()),
                            )
                    })),
            )
            .into_any_element()
    }

    fn render_validate(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let v = &self.validation;
        let muted = cx.theme().muted_foreground;
        let errors = v.issues.iter().filter(|i| i.level == "error").count();
        let warns = v.issues.len() - errors;
        let command = format!("requestbaby validate -e {}", self.env);
        let toolbar = h_flex()
            .px_3()
            .py_2()
            .gap_2()
            .child(div().text_xs().font_weight(FontWeight::MEDIUM).child(format!("项目校验 · 环境 {}", self.env)))
            .map(|this| match (&v.error, v.running && !v.done) {
                (Some(_), _) => this.child(Tag::danger().outline().xsmall().child("校验出错")),
                (_, true) => this.child(Tag::secondary().outline().xsmall().child("校验中…")),
                _ if errors + warns == 0 => this.child(Tag::success().outline().xsmall().child("全部通过")),
                _ => this
                    .when(errors > 0, |t| t.child(Tag::danger().outline().xsmall().child(format!("{errors} 个错误"))))
                    .when(warns > 0, |t| t.child(Tag::warning().outline().xsmall().child(format!("{warns} 个警告")))),
            })
            .child(div().flex_1())
            .child(div().text_xs().font_family(cx.theme().mono_font_family.clone()).text_color(muted).child(command.clone()))
            .child(Clipboard::new("copy-validate").value(SharedString::from(command)))
            .child(
                Button::new("revalidate")
                    .xsmall()
                    .outline()
                    .icon(IconName::RefreshCw)
                    .label("重新校验")
                    .on_click(cx.listener(|this, _, _, cx| this.revalidate(cx))),
            )
            .child(
                Button::new("fmt-project")
                    .xsmall()
                    .outline()
                    .icon(IconName::FileText)
                    .label("格式化")
                    .tooltip("requestbaby fmt：规范字段顺序，公共请求头上提到 collection.json")
                    .on_click(cx.listener(|this, _, window, cx| this.format_project(window, cx))),
            )
            .child(
                Button::new("doctor")
                    .xsmall()
                    .outline()
                    .icon(IconName::Settings)
                    .label("环境自检")
                    .tooltip("requestbaby doctor")
                    .on_click(cx.listener(|this, _, window, cx| this.run_doctor(window, cx))),
            );

        let body: AnyElement = if let Some(e) = &v.error {
            div().px_4().text_xs().text_color(cx.theme().danger).child(e.clone()).into_any_element()
        } else if v.issues.is_empty() {
            empty_state(
                IconName::CircleCheck,
                "结构、变量引用、脚本语法、工作流目标全部通过 —— 与 CLI requestbaby validate 同一实现".into(),
                cx,
            )
        } else {
            let root = self.project.root().to_path_buf();
            v_flex()
                .id("validate-list")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px_2()
                .children(v.issues.iter().enumerate().map(|(ix, i)| {
                    let color = if i.level == "error" { cx.theme().danger } else { cx.theme().warning };
                    let path = PathBuf::from(&i.file);
                    let rel = self.project.rel_of(&path);
                    let path = if path.is_absolute() { path } else { root.join(&path) };
                    h_flex()
                        .id(("issue", ix))
                        .px_2()
                        .py_1()
                        .gap_3()
                        .items_start()
                        .text_xs()
                        .rounded(cx.theme().radius)
                        .hover(|this| this.bg(cx.theme().table_hover))
                        .on_click(cx.listener(move |this, _, window, cx| this.open_path(&path, window, cx)))
                        .child(Icon::new(IconName::TriangleAlert).xsmall().text_color(color))
                        .child(div().w(px(300.)).flex_shrink_0().overflow_hidden().text_ellipsis().child(rel))
                        .child(div().w(px(120.)).flex_shrink_0().text_color(muted).child(i.rule))
                        .child(div().flex_1().min_w_0().child(i.message.clone()))
                }))
                .into_any_element()
        };
        v_flex().flex_1().min_h_0().child(toolbar).child(body).into_any_element()
    }

    pub(super) fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let last = self.project.history().first().map(|r| {
            let m = r.meta.clone().unwrap_or_default();
            format!("最近运行 {} · {}/{} 通过 · {}", m.source, r.passed, r.total, relative_time(m.started_at_ms))
        });
        StatusBar::new()
            .left(
                h_flex()
                    .gap_3()
                    .text_xs()
                    .text_color(muted)
                    .child(h_flex().gap_1().child(Icon::new(IconName::FolderOpen).xsmall()).child(self.project.root().display().to_string()))
                    .child(
                        h_flex()
                            .gap_1()
                            .child(Icon::new(IconName::RefreshCw).xsmall())
                            .child(if self._watcher.is_some() { "监听文件变更（CLI / AI 写入实时同步）" } else { "文件监听不可用" }),
                    ),
            )
            .right(
                h_flex()
                    .gap_3()
                    .text_xs()
                    .text_color(muted)
                    .when_some(last, |this, l| this.child(l))
                    .child(format!("libcurl {}", requestbaby_core::engine::libcurl_version())),
            )
    }
}

/// 结果表列宽：图标 | 用例 | 文件(弹性) | 方法 | 状态 | 耗时 | 断言
fn result_cols(row: Stateful<Div>, cells: [AnyElement; 7]) -> Stateful<Div> {
    let [icon, name, file, method, status, dur, tests] = cells;
    row.child(div().w(px(28.)).flex_shrink_0().child(icon))
        .child(div().w(px(240.)).flex_shrink_0().overflow_hidden().text_ellipsis().child(name))
        .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().child(file))
        .child(div().w(px(64.)).flex_shrink_0().child(method))
        .child(div().w(px(56.)).flex_shrink_0().flex().justify_end().child(status))
        .child(div().w(px(80.)).flex_shrink_0().flex().justify_end().child(dur))
        .child(div().w(px(64.)).flex_shrink_0().flex().justify_end().child(tests))
}

fn empty_state(icon: IconName, hint: String, cx: &Context<Workspace>) -> AnyElement {
    v_flex()
        .flex_1()
        .items_center()
        .justify_center()
        .gap_2()
        .text_color(cx.theme().muted_foreground)
        .child(Icon::new(icon).large())
        .child(div().text_xs().child(hint))
        .into_any_element()
}
