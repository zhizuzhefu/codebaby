//! 底部面板：运行结果（含流程步骤、历史运行切换与报告导出），以及状态栏。
//! 脚本输出、配置问题只在有内容时才从这里进入，不常驻为标签页。
use crate::style;
use super::{BottomTab, Workspace, run_origin};
use crate::project::{CaseStatus, status_of};
use requestbaby_core::config::Protocol;
use crate::run::{Phase, ResultsSource, StepState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex,
    button::{Button, ButtonVariants as _},
    menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem},
    notification::Notification,
    progress::Progress,
    status_bar::StatusBar,
    stepper::{Stepper, StepperItem},
    tag::Tag,
    tooltip::Tooltip,
    v_flex,
};
use gpui_kit::{
    AnyElement, ClipboardItem, Context, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Stateful, StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px, uniform_list,
};
use requestbaby_core::history;
use requestbaby_core::validate::ValidationIssue;
use requestbaby_core::report::{CaseReport, CompareMark, Comparison};
use requestbaby_core::timeline::{self, EntryKind, EntryStatus, TimelineEntry};
use std::path::PathBuf;
use std::rc::Rc;

impl Workspace {
    pub(super) fn render_bottom(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex().size_full().map(|this| match self.bottom_tab {
            BottomTab::Results => this.child(self.render_results(cx)),
            BottomTab::Logs => this.child(self.render_logs(cx)),
            BottomTab::Issues => this.child(self.render_validate(cx)),
        })
    }

    /// 脚本输出 / 配置问题页顶部：标题 + 返回运行结果
    fn sub_page_bar(&self, title: &'static str, cx: &mut Context<Self>) -> gpui_kit::Div {
        h_flex()
            .px_3()
            .py_2()
            .gap_2()
            .child(
                Button::new("back-to-results")
                    .xsmall()
                    .ghost()
                    .icon(IconName::ArrowLeft)
                    .label("运行结果")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.bottom_tab = BottomTab::Results;
                        cx.notify();
                    })),
            )
            .child(div().text_xs().font_weight(FontWeight::MEDIUM).child(title))
    }

    fn render_results(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let run = self.run.read(cx);
        let rows = run.rows();
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
        let is_verify = run.is_verify();
        // 界面用例进行中：当前做到哪一步
        let log_count = run.logs().len();
        let live_text = run.live().and_then(|l| {
            let step = l.steps.last()?;
            let name = l.file.rsplit('/').next().unwrap_or(&l.file).trim_end_matches(".json");
            Some(format!("正在运行 {name} · 第 {} 步 {}", step.index, step.title))
        });
        let dry_run = run.report().and_then(|r| r.meta.as_ref()).is_some_and(|m| m.dry_run);
        let compare = if running { None } else { run.report().and_then(|r| r.compare.clone()).filter(|c| !c.is_empty()) };
        let summary = if running { String::new() } else { run.report().map(|r| r.failure_text()).unwrap_or_default() };
        let blocked = if running { None } else { run.report().and_then(|r| r.blocked.clone()) };
        let failed_files: Vec<String> = {
            let mut f: Vec<String> = rows.iter().filter(|r| !r.passed && !r.skipped).map(|r| r.file.clone()).collect();
            f.dedup();
            f
        };
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
        // 一次运行里有两种以上的端（API / Web / Android…）才提供时间线
        let mixed = timeline::sides(rows).len() > 1;
        let show_timeline = mixed && self.results_timeline;

        if rows.is_empty() && steps.is_empty() && !running {
            let hint = match phase {
                Phase::Failed(e) => e,
                _ => "还没有运行结果。点击右上角「运行全部」或「完整验证」，结果会显示在这里".into(),
            };
            return empty_state(IconName::SquareTerminal, hint, cx);
        }

        let header = h_flex()
            .px_3()
            .py_2()
            .gap_3()
            .child(self.history_button(
                match (&workflow, is_verify) {
                    (_, true) => format!("{origin} · 完整验证"),
                    (Some(w), _) => format!("{origin} · 流程 {w}"),
                    _ => origin,
                },
                cx,
            ))
            .when(mixed, |this| {
                this.child(
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new("view-cases")
                                .xsmall()
                                .when(show_timeline, |b| b.ghost())
                                .when(!show_timeline, |b| b.primary())
                                .label("按用例")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.results_timeline = false;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("view-timeline")
                                .xsmall()
                                .when(show_timeline, |b| b.primary())
                                .when(!show_timeline, |b| b.ghost())
                                .label("时间线")
                                .tooltip("API / Web / Android / Mac 桌面的用例与步骤，按真实发生的先后排在一起")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.results_timeline = true;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .when(dry_run, |this| this.child(Tag::info().outline().xsmall().child("dry-run")))
            .child(Tag::success().outline().xsmall().child(format!("{passed} 通过")))
            .when(failed > 0, |this| this.child(Tag::danger().outline().xsmall().child(format!("{failed} 失败"))))
            .when(skipped > 0, |this| this.child(Tag::secondary().outline().xsmall().child(format!("{skipped} 跳过"))))
            .when_some(compare.clone(), |this, c| {
                // 与历史对比：先看新失败（回归）
                this.child(div().text_xs().text_color(muted).child("对比上次"))
                    .when(!c.new_failures.is_empty(), |this| this.child(Tag::danger().xsmall().child(format!("新失败 {}", c.new_failures.len()))))
                    .when(!c.fixed.is_empty(), |this| this.child(Tag::success().xsmall().child(format!("已修复 {}", c.fixed.len()))))
                    .when(!c.still_failing.is_empty(), |this| this.child(Tag::warning().xsmall().child(format!("仍失败 {}", c.still_failing.len()))))
            })
            .when(!running && !dry_run && !failed_files.is_empty(), |this| {
                let files = failed_files.clone();
                this.child(
                    Button::new("rerun-failed")
                        .xsmall()
                        .outline()
                        .icon(IconName::RotateCw)
                        .label("重跑失败")
                        .tooltip("只重跑失败的用例文件，并自动带上它们依赖的前置用例（如捕获 token 的登录）")
                        .on_click(cx.listener(move |this, _, window, cx| this.rerun_failed(files.clone(), window, cx))),
                )
            })
            .when(!summary.is_empty(), |this| {
                this.child(
                    Button::new("copy-failures")
                        .xsmall()
                        .outline()
                        .icon(IconName::Copy)
                        .label("复制失败摘要")
                        .tooltip("复制本次所有失败（错误信息、失败断言、无法完成的原因）为纯文本，可直接贴给 AI 或同事")
                        .on_click(move |_, window, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(summary.clone()));
                            window.push_notification(Notification::success("已复制失败摘要"), cx);
                        }),
                )
            })
            .child(div().text_xs().text_color(muted).child(format!("{elapsed} ms")))
            .when(running && planned > 0, |this| {
                let pct = (rows.len() as f32 / planned as f32 * 100.).min(100.);
                this.child(div().w(px(160.)).child(Progress::new("run-progress").value(pct)))
            })
            .when_some(live_text, |this, t| this.child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().text_xs().text_color(cx.theme().primary).child(t)))
            .child(div().flex_1())
            .when(log_count > 0, |this| {
                this.child(
                    Button::new("open-logs")
                        .xsmall()
                        .ghost()
                        .icon(IconName::FileText)
                        .label(format!("脚本输出 {log_count}"))
                        .tooltip("用例脚本里 log(...) 打印的内容")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.bottom_tab = BottomTab::Logs;
                            cx.notify();
                        })),
                )
            })
            .when(!command.is_empty(), |this| this.child(style::cli_button("results-cli", command.clone())))
            .child(
                Button::new("export-report")
                    .xsmall()
                    .outline()
                    .icon(IconName::ExternalLink)
                    .label("导出结果")
                    .tooltip("导出验收结果给别人（⌘E · requestbaby export）")
                    .disabled(running)
                    .on_click(cx.listener(|this, _, window, cx| this.open_export_dialog(window, cx))),
            );

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
        let timeline_body = show_timeline.then(|| self.timeline_list(Rc::new(timeline::timeline(rows)), cx));
        let row_count = rows.len();
        let body = uniform_list(
            "results-list",
            row_count,
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                let run = this.run.read(cx);
                let compare = if run.is_streaming() { None } else { run.report().and_then(|r| r.compare.as_ref()).filter(|c| !c.is_empty()) };
                run.rows()[range.start.min(run.rows().len())..range.end.min(run.rows().len())]
                    .iter()
                    .enumerate()
                    .map(|(i, r)| this.result_row(range.start + i, r, compare, dry_run, cx))
                    .collect::<Vec<_>>()
            }),
        )
        .size_full();

        v_flex()
            .flex_1()
            .min_h_0()
            .child(header)
            // 时间线里每个用例已按先后排开，流程阶段条就不再占地方
            .when(!steps.is_empty() && !show_timeline, |this| {
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
            .when_some(blocked, |this, why| {
                // 无法给出结论的原因：完整显示（可能多行），点击复制
                let text = why.clone();
                this.child(
                    h_flex()
                        .id("results-blocked")
                        .mx_2()
                        .mb_2()
                        .px_2()
                        .py_1()
                        .gap_2()
                        .items_start()
                        .rounded(cx.theme().radius)
                        .border_1()
                        .border_color(cx.theme().danger)
                        .cursor_pointer()
                        .hover(|d| d.bg(cx.theme().muted))
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(Icon::new(IconName::TriangleAlert).xsmall())
                        .child(div().flex_1().min_w_0().child(format!("无法完成：{why}")))
                        .child(Icon::new(IconName::Copy).xsmall())
                        .tooltip(|window, cx| Tooltip::new("点击复制").build(window, cx))
                        .on_click(move |_, window, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                            window.push_notification(Notification::success("已复制"), cx);
                        }),
                )
            })
            .map(|this| match timeline_body {
                Some(t) => this.child(div().flex_1().min_h_0().px_2().child(t)),
                None => this.child(div().px_2().child(head)).child(div().flex_1().min_h_0().px_2().child(body)),
            })
            .into_any_element()
    }

    /// 结果表的一行（虚拟列表按可见范围调用；右键菜单打开时再取数据，不为每行拷贝报告）
    fn result_row(&self, ix: usize, r: &CaseReport, compare: Option<&Comparison>, dry_run: bool, cx: &Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let (hover, active, selected_row) = (cx.theme().table_hover, cx.theme().table_active, self.selected_row);
        let (run, file) = (self.run.clone(), r.file.clone());
        let (icon, color) = match status_of(r) {
            CaseStatus::Passed => (IconName::CircleCheck, cx.theme().success),
            CaseStatus::Failed => (IconName::CircleX, cx.theme().danger),
            CaseStatus::Skipped => (IconName::Ban, muted),
        };
        let tests_pass = r.tests.iter().filter(|t| t.pass).count();
        // 报告给 gRPC / 界面用例存状态文字（"-" = 没拿到响应）；HTTP / WebSocket 的 0 = 没拿到响应
        let responded = match r.protocol {
            Protocol::Grpc | Protocol::Web | Protocol::Android | Protocol::Desktop => !r.status_text.is_empty() && r.status_text != "-",
            _ => r.status > 0,
        };
        let status = requestbaby_core::runner::status_text(r.protocol, r.status, responded);
        let bad = requestbaby_core::runner::status_is_bad(r.protocol, r.status, responded);
        let status_color = if bad && (responded || !dry_run) { cx.theme().danger } else { cx.theme().foreground };
        let tests_color = if tests_pass < r.tests.len() { cx.theme().danger } else { cx.theme().foreground };
        let row = h_flex()
            .id(("result-row", ix))
            // 虚拟列表要求行高一致；行宽撑满，弹性列才会按容器宽度省略
            .w_full()
            .h(px(28.))
            .px_2()
            .gap_2()
            .text_xs()
            .border_b_1()
            .border_color(cx.theme().table_row_border)
            .when(selected_row == Some(ix), |this| this.bg(active))
            .hover(|this| this.bg(hover))
            .on_click(cx.listener(move |this, _, window, cx| this.select_row(ix, window, cx)));
        let mark = compare.as_ref().and_then(|c| c.mark(&r.file, &r.name));
        result_cols(
            row,
            [
                Icon::new(icon).xsmall().text_color(color).into_any_element(),
                h_flex()
                    .gap_1()
                    .min_w_0()
                    .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().font_weight(FontWeight::MEDIUM).child(r.name.clone()))
                    .when_some(mark, |this, m| {
                        this.child(match m {
                            CompareMark::NewFailure => Tag::danger().xsmall().flex_shrink_0().child(m.label()),
                            CompareMark::Fixed => Tag::success().xsmall().flex_shrink_0().child(m.label()),
                            CompareMark::StillFailing => Tag::warning().outline().xsmall().flex_shrink_0().child(m.label()),
                        })
                    })
                    .into_any_element(),
                div().text_color(muted).child(r.file.clone()).into_any_element(),
                style::method_label(&r.method, cx).into_any_element(),
                div().text_color(status_color).child(status).into_any_element(),
                div().text_color(muted).child(format!("{} ms", r.duration_ms)).into_any_element(),
                div()
                    .text_color(tests_color)
                    .child(if r.tests.is_empty() { "-".to_string() } else { format!("{tests_pass}/{}", r.tests.len()) })
                    .into_any_element(),
            ],
        )
        .context_menu(move |menu, _, cx| match run.read(cx).rows().get(ix).filter(|r| r.file == file) {
            Some(r) => row_menu(menu, r),
            None => menu,
        })
        .into_any_element()
    }

    /// 时间线：所有端的用例（界面用例展开成每一步）按真实先后排在一起；点一行打开对应用例的结果
    fn timeline_list(&self, entries: Rc<Vec<TimelineEntry>>, cx: &Context<Self>) -> AnyElement {
        let n = entries.len();
        uniform_list(
            "timeline-list",
            n,
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                let end = range.end.min(entries.len());
                entries[range.start.min(end)..end].iter().enumerate().map(|(i, e)| this.timeline_row(range.start + i, e, cx)).collect::<Vec<_>>()
            }),
        )
        .size_full()
        .into_any_element()
    }

    fn timeline_row(&self, ix: usize, e: &TimelineEntry, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let (icon, color) = match e.status {
            EntryStatus::Passed => (IconName::CircleCheck, theme.success),
            EntryStatus::Failed => (IconName::CircleX, theme.danger),
            EntryStatus::Skipped => (IconName::Ban, muted),
            EntryStatus::Info => (IconName::FileText, muted),
        };
        let side = match e.protocol {
            Protocol::Http | Protocol::Grpc | Protocol::Ws => Tag::info(),
            Protocol::Web => Tag::primary(),
            Protocol::Android => Tag::secondary(),
            Protocol::Desktop => Tag::warning(),
        }
        .outline()
        .xsmall()
        .flex_shrink_0()
        .child(timeline::side_label(e.protocol));
        let first_detail = e.detail.lines().next().unwrap_or("").to_string();
        let case = e.case;
        let selected = self.selected_row == Some(case);
        let (hover, active) = (theme.table_hover, theme.table_active);
        let text = match e.kind {
            EntryKind::Log => div().text_color(muted).child(format!("输出  {first_detail}")).into_any_element(),
            kind => h_flex()
                .gap_2()
                .min_w_0()
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .when(kind == EntryKind::Case, |d| d.font_weight(FontWeight::MEDIUM))
                        .child(e.text.clone()),
                )
                .when(!first_detail.is_empty(), |this| {
                    this.child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(if e.status == EntryStatus::Failed { theme.danger } else { muted })
                            .child(first_detail.clone()),
                    )
                })
                .into_any_element(),
        };
        h_flex()
            .id(("timeline-row", ix))
            .w_full()
            .h(px(26.))
            .px_2()
            .gap_2()
            .text_xs()
            .when(e.kind == EntryKind::Case, |d| d.border_t_1().border_color(theme.table_row_border))
            .when(selected, |d| d.bg(active))
            .hover(|d| d.bg(hover))
            .on_click(cx.listener(move |this, _, window, cx| this.select_row(case, window, cx)))
            .child(div().w(px(56.)).flex_shrink_0().flex().justify_end().text_color(muted).child(format!("+{:.1}s", e.at_ms as f64 / 1000.)))
            .child(div().w(px(64.)).flex_shrink_0().child(side))
            .when(e.kind != EntryKind::Case, |d| d.child(div().w(px(14.)).flex_shrink_0()))
            .child(Icon::new(icon).xsmall().text_color(color).flex_shrink_0())
            .child(div().flex_1().min_w_0().child(text))
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
        let bar = self.sub_page_bar("脚本输出 · 用例脚本里 log(...) 打印的内容", cx);
        if logs.is_empty() {
            return v_flex().flex_1().min_h_0().child(bar).child(empty_state(IconName::FileText, "这次运行里，脚本没有打印任何内容".into(), cx)).into_any_element();
        }
        v_flex().flex_1().min_h_0().child(bar).child(v_flex()
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
            })))
            .into_any_element()
    }

    /// 结果顶部的「来源」按钮：显示正在看的是哪次运行，点开切换到历史运行（CLI 与桌面端共享同一份记录）
    fn history_button(&self, label: String, cx: &Context<Self>) -> AnyElement {
        let runs = self.project.history();
        if runs.is_empty() {
            return div().text_xs().font_weight(FontWeight::MEDIUM).child(label).into_any_element();
        }
        let current = match self.run.read(cx).source() {
            ResultsSource::History(id) => Some(id.clone()),
            _ => None,
        };
        let items: Vec<(String, String, bool, bool)> = runs
            .iter()
            .take(20)
            .filter_map(|r| {
                let m = r.meta.as_ref()?;
                let text = format!("{} · {}/{} 通过", run_origin(r), r.passed, r.total);
                Some((m.id.clone(), text, r.failed > 0, current.as_deref() == Some(m.id.as_str())))
            })
            .collect();
        let total = runs.len();
        let view = cx.entity().downgrade();
        Button::new("history-menu")
            .xsmall()
            .ghost()
            .label(label)
            .icon(IconName::ChevronDown)
            .tooltip("切换查看以前的运行结果")
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu.item(PopupMenuItem::label(format!("历史运行（共 {total} 条，最近 20 条）")));
                for (id, text, failed, is_current) in items.iter().cloned() {
                    let view = view.clone();
                    menu = menu.item(
                        PopupMenuItem::new(text)
                            .icon(if failed { IconName::CircleX } else { IconName::CircleCheck })
                            .checked(is_current)
                            .on_click(move |_, _, cx| {
                                let _ = view.update(cx, |w, cx| w.show_history_run(&id, cx));
                            }),
                    );
                }
                let clean = |label: &'static str, c: history::Clean, title: &'static str| {
                    super::sidebar::menu_item(&view, label, IconName::Delete, move |w, window, cx| w.clean_history(c, title, window, cx))
                };
                menu.separator()
                    .item(clean("只保留最近 20 条", history::Clean { keep: Some(20), ..Default::default() }, "只保留最近 20 条运行记录？"))
                    .item(clean("删除 7 天前的", history::Clean { older_than_ms: Some(7 * 86_400_000), ..Default::default() }, "删除 7 天前的运行记录？"))
                    .item(clean("清空全部历史…", history::Clean::default(), "清空全部运行历史？"))
            })
            .into_any_element()
    }

    /// 在结果页显示某次历史运行
    fn show_history_run(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(report) = self.project.history().iter().find(|r| r.meta.as_ref().is_some_and(|m| m.id == id)).cloned() else { return };
        self.run.update(cx, |run, cx| run.show_report(&report, cx));
        self.selected_row = None;
        self.bottom_tab = BottomTab::Results;
        cx.notify();
    }

    fn render_validate(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let v = &self.validation;
        let muted = cx.theme().muted_foreground;
        // 写法建议不算「警告」：不影响结论，不逐条给人看（见底部的提示行）
        let shown: Vec<&ValidationIssue> = v.issues.iter().filter(|i| !i.is_style()).collect();
        let errors = shown.iter().filter(|i| i.level == "error").count();
        let warns = shown.len() - errors;
        let notice = style::style_notice("copy-ai-brief", &v.issues, &self.env, cx);
        let command = format!("requestbaby validate -e {}", self.env);
        let toolbar = self
            .sub_page_bar("配置检查", cx)
            .child(div().text_xs().text_color(muted).child(format!("环境 {}", self.env)))
            .map(|this| match (&v.error, v.running && !v.done) {
                (Some(_), _) => this.child(Tag::danger().outline().xsmall().child("检查出错")),
                (_, true) => this.child(Tag::secondary().outline().xsmall().child("检查中…")),
                _ if errors + warns == 0 => this.child(Tag::success().outline().xsmall().child("全部通过")),
                _ => this
                    .when(errors > 0, |t| t.child(Tag::danger().outline().xsmall().child(format!("{errors} 个错误"))))
                    .when(warns > 0, |t| t.child(Tag::warning().outline().xsmall().child(format!("{warns} 个提醒")))),
            })
            .child(div().flex_1())
            .child(style::cli_button("validate-cli", command))
            .child(
                Button::new("revalidate")
                    .xsmall()
                    .outline()
                    .icon(IconName::RefreshCw)
                    .label("重新检查")
                    .on_click(cx.listener(|this, _, _, cx| this.revalidate(cx))),
            );

        let body: AnyElement = if let Some(e) = &v.error {
            div().px_4().text_xs().text_color(cx.theme().danger).child(e.clone()).into_any_element()
        } else if shown.is_empty() {
            empty_state(
                IconName::CircleCheck,
                "结构、变量引用、脚本语法、流程目标全部通过".into(),
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
                .children(shown.iter().enumerate().map(|(ix, i)| {
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
        v_flex()
            .flex_1()
            .min_h_0()
            .child(toolbar)
            .child(body)
            .when_some(notice, |this, n| this.child(div().px_4().py_2().border_t_1().border_color(cx.theme().border).child(n)))
            .into_any_element()
    }

    pub(super) fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let v = &self.validation;
        let problems = v.issues.iter().filter(|i| !i.is_style()).count() + usize::from(v.error.is_some());
        let proxy_on = self.proxy_mode() == requestbaby_core::proxy_setting::Mode::On;
        let session = self.run.read(cx).session();
        let has_session = !session.vars.is_empty() || !session.cookies.is_empty();
        StatusBar::new()
            .left(
                h_flex()
                    .gap_3()
                    .text_xs()
                    .text_color(muted)
                    .child(h_flex().gap_1().child(Icon::new(IconName::FolderOpen).xsmall()).child(self.project.root().display().to_string()))
                    .when(problems > 0, |this| {
                        this.child(
                            Button::new("open-issues")
                                .xsmall()
                                .ghost()
                                .icon(IconName::TriangleAlert)
                                .text_color(cx.theme().warning)
                                .label(format!("{problems} 处配置问题"))
                                .tooltip("查看哪里需要修改")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.bottom_tab = BottomTab::Issues;
                                    cx.notify();
                                })),
                        )
                    })
                    .when(self._watcher.is_none(), |this| this.child("文件监听不可用：外部改动需重新打开项目")),
            )
            .right(
                h_flex()
                    .gap_3()
                    .text_xs()
                    .text_color(muted)
                    .when(proxy_on, |this| this.child(h_flex().gap_1().child(Icon::new(IconName::Network).xsmall()).child("代理已开启")))
                    .when_some(self.render_update_badge(cx), |this, b| this.child(b))
                    .when(has_session, |this| this.child(self.render_session(cx))),
            )
    }
}

impl Workspace {
    /// 本次会话（捕获的变量 + Cookie）：「发送」单个用例时带上，可单独调试链路中的任意一步
    fn render_session(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let session = self.run.read(cx).session();
        let (nv, nc) = (session.vars.len(), session.cookies.len());
        let mut lines: Vec<String> = session
            .vars
            .iter()
            .map(|(k, v)| {
                let v = v.to_render();
                let short: String = v.chars().take(48).collect();
                format!("{k} = {short}{}", if v.chars().count() > 48 { "…" } else { "" })
            })
            .collect();
        lines.extend(session.cookies.iter().map(|c| format!("🍪 {}={} · {}{}", c.name, c.value.chars().take(32).collect::<String>(), c.domain, c.path)));
        let view = cx.entity().downgrade();
        Button::new("session-state")
            .xsmall()
            .ghost()
            .icon(IconName::Inbox)
            .label(if nv + nc == 0 { "会话：空".to_string() } else { format!("会话：变量 {nv} · Cookie {nc}") })
            .tooltip("本次会话中运行 / 发送捕获的变量与 Cookie；单独「发送」某个用例时会带上它们")
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                if lines.is_empty() {
                    menu = menu.item(PopupMenuItem::new("还没有捕获：运行或发送带 post 捕获的用例（如登录）后出现在这里").disabled(true));
                }
                for l in lines.iter().take(30) {
                    let text = l.clone();
                    menu = menu.item(PopupMenuItem::new(text.clone()).on_click(move |_, window, cx| {
                        let value = text.split_once(" = ").map(|(_, v)| v.to_string()).unwrap_or(text.clone());
                        cx.write_to_clipboard(ClipboardItem::new_string(value));
                        window.push_notification(Notification::success("已复制"), cx);
                    }));
                }
                menu.separator().item(super::sidebar::menu_item(&view, "清空会话", IconName::Delete, |w, window, cx| w.clear_session(window, cx)))
            })
    }
}

impl Workspace {
    fn clean_history(&mut self, clean: history::Clean, title: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let (root, this) = (self.project.root().to_path_buf(), cx.entity().downgrade());
        crate::dialogs::confirm_danger(
            window,
            cx,
            title,
            "运行记录直接删除（不进废纸篓）。「对比上次」「重跑失败」与用例树上的状态都基于剩余的历史。",
            "删除",
            move |window, cx| {
                let _ = this.update(cx, |w, cx| match history::clean(&root, clean) {
                    Ok(n) => {
                        w.after_history_cleaned(window, cx);
                        window.push_notification(Notification::success(format!("已清理 {n} 条运行记录")), cx);
                    }
                    Err(e) => window.push_notification(Notification::error(format!("{e:#}")), cx),
                });
            },
        );
    }

    /// 清理后：刷新列表；结果区正展示的记录已被删除时改为展示最近一条，一条不剩则清空结果区与响应面板里的历史残留
    fn after_history_cleaned(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reload_history(cx);
        let gone = match self.run.read(cx).source() {
            ResultsSource::History(id) => !self.project.history().iter().any(|r| r.meta.as_ref().is_some_and(|m| &m.id == id)),
            ResultsSource::Live => false,
        };
        match self.project.history().first().cloned() {
            Some(latest) if gone => {
                self.run.update(cx, |r, cx| r.show_report(&latest, cx));
                self.selected_row = None;
            }
            None => {
                self.run.update(cx, |r, cx| r.clear_results(cx));
                self.selected_row = None;
            }
            _ => {}
        }
        self.refresh_open_response(window, cx);
        cx.notify();
    }
}

/// 结果行右键：复制 URL / cURL / 文件路径 / 失败摘要（贴给 AI 或同事）
fn row_menu(menu: PopupMenu, r: &CaseReport) -> PopupMenu {
    let copy = |label: String, what: &'static str, text: String| {
        PopupMenuItem::new(label).icon(IconName::Copy).on_click(move |_, window, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
            window.push_notification(Notification::success(format!("已复制{what}")), cx);
        })
    };
    let mut menu = menu
        .item(copy(format!("复制地址（{}）", r.method), "地址", r.url.clone()))
        .item(copy("复制用例文件路径".into(), "文件路径", r.file.clone()));
    if let Some(c) = &r.curl {
        let tool = match r.protocol {
            Protocol::Http => "cURL",
            Protocol::Grpc => "grpcurl",
            Protocol::Ws => "websocat",
            Protocol::Web | Protocol::Android | Protocol::Desktop => "等价",
        };
        menu = menu.item(copy(format!("复制为 {tool} 命令"), "命令", c.clone()));
    }
    if !r.passed && !r.skipped {
        menu = menu.separator().item(copy("复制失败摘要".into(), "失败摘要", r.failure_text()));
    }
    menu
}

/// 结果表列宽：图标 | 用例 | 文件(弹性) | 方法 | 状态 | 耗时 | 断言
fn result_cols(row: Stateful<Div>, cells: [AnyElement; 7]) -> Stateful<Div> {
    let [icon, name, file, method, status, dur, tests] = cells;
    row.child(div().w(px(28.)).flex_shrink_0().child(icon))
        .child(div().w(px(240.)).flex_shrink_0().overflow_hidden().whitespace_nowrap().child(name))
        .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(file))
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
