//! 导出结果：选范围（各用例的最新结果 —— 可勾选一个或多个集合 / 当前显示的这一次运行）和格式，
//! 先看清报告里有什么，再导出。与 `requestbaby export` 同一实现，并实时展示等价命令。
use crate::style;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, button::{Button, ButtonVariants as _}, checkbox::Checkbox, h_flex, radio::RadioGroup, v_flex};
use gpui_kit::{
    Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};
use requestbaby_core::export::{Format, Overview};
use requestbaby_core::report::RunReport;
use std::path::Path;

/// 一个有结果的集合（可勾选）
pub struct CollectionItem {
    dir: String,
    cases: usize,
    overview: Overview,
    checked: bool,
}

impl CollectionItem {
    pub fn new(dir: String, cases: usize, report: &RunReport, root: &Path) -> Self {
        let overview = Overview::of(report, root, std::slice::from_ref(&dir));
        Self { dir, cases, overview, checked: true }
    }
}

/// 当前结果面板显示的那一次运行
pub struct RunChoice {
    title: String,
    report: RunReport,
    overview: Overview,
    id: String,
}

impl RunChoice {
    pub fn new(title: String, report: RunReport, root: &Path, id: String) -> Self {
        let overview = Overview::of(&report, root, &[]);
        Self { title, report, overview, id }
    }
}

/// 导出对话框的结果
pub enum Selection {
    /// 环境下各用例的最新结果；集合目录为空 = 全部
    Latest(Vec<String>),
    Run(Box<RunReport>),
}

pub struct ExportPanel {
    env: String,
    items: Vec<CollectionItem>,
    run: Option<RunChoice>,
    /// 选的是「当前这一次运行」
    use_run: bool,
    format: Format,
}

impl ExportPanel {
    pub fn new(env: String, items: Vec<CollectionItem>, run: Option<RunChoice>) -> Self {
        let use_run = items.is_empty() && run.is_some();
        Self { env, items, run, use_run, format: Format::Html }
    }

    fn picked(&self) -> Vec<&CollectionItem> {
        self.items.iter().filter(|i| i.checked).collect()
    }

    pub fn selection(&self) -> Result<(Selection, Format), &'static str> {
        let format = self.format;
        if self.use_run {
            let r = self.run.as_ref().ok_or("没有可导出的运行")?;
            return Ok((Selection::Run(Box::new(r.report.clone())), format));
        }
        let picked = self.picked();
        if picked.is_empty() {
            return Err("请至少勾选一个集合");
        }
        // 全选 = 全部（也包含不在任何集合里的根级用例）
        let dirs = if picked.len() == self.items.len() { vec![] } else { picked.iter().map(|i| i.dir.clone()).collect() };
        Ok((Selection::Latest(dirs), format))
    }

    fn command(&self) -> String {
        let format = if self.format == Format::Html { String::new() } else { format!(" -f {}", self.format.name()) };
        if self.use_run {
            return format!("requestbaby export {}{format}", self.run.as_ref().map(|r| r.id.as_str()).unwrap_or(""));
        }
        let picked = self.picked();
        let cs: String = if picked.len() == self.items.len() { String::new() } else { picked.iter().map(|i| format!(" -c {}", i.dir)).collect() };
        format!("requestbaby export -e {}{cs}{format}", self.env)
    }

    fn overview(&self) -> Overview {
        if self.use_run {
            if let Some(r) = &self.run {
                return Overview::combine(&[&r.overview]);
            }
        }
        let parts: Vec<&Overview> = self.picked().iter().map(|i| &i.overview).collect();
        Overview::combine(&parts)
    }
}

impl Render for ExportPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (muted, ok, bad) = (cx.theme().muted_foreground, cx.theme().success, cx.theme().danger);
        let label = |t: &'static str| div().text_xs().font_weight(FontWeight::MEDIUM).text_color(muted).child(t);
        let overview = self.overview();
        let notes = if self.use_run || !self.picked().is_empty() { overview.notes() } else { vec![] };
        let (picked, total) = (self.picked().len(), self.items.len());

        let scope = self.run.as_ref().filter(|_| !self.items.is_empty()).map(|r| {
            RadioGroup::vertical("export-scope")
                .children([format!("各用例的最新结果（环境 {}）", self.env), r.title.clone()])
                .selected_index(Some(self.use_run as usize))
                .on_change(cx.listener(|this, ix: &usize, _, cx| {
                    this.use_run = *ix == 1;
                    cx.notify();
                }))
        });

        let list = (!self.use_run).then(|| {
            v_flex()
                .gap_1()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(label("集合（可多选）"))
                        .child(div().text_xs().text_color(muted).child(format!("已选 {picked} / {total}")))
                        .child(div().flex_1())
                        .child(Button::new("export-all").xsmall().ghost().label("全选").on_click(cx.listener(|this, _, _, cx| {
                            this.items.iter_mut().for_each(|i| i.checked = true);
                            cx.notify();
                        })))
                        .child(Button::new("export-none").xsmall().ghost().label("全不选").on_click(cx.listener(|this, _, _, cx| {
                            this.items.iter_mut().for_each(|i| i.checked = false);
                            cx.notify();
                        }))),
                )
                .child(
                    v_flex()
                        .id("export-collections")
                        .max_h(px(260.))
                        .overflow_y_scroll()
                        .gap_1()
                        .p_2()
                        .border_1()
                        .border_color(cx.theme().border)
                        .rounded(cx.theme().radius)
                        .children(self.items.iter().enumerate().map(|(ix, i)| {
                            let state = if i.overview.failed > 0 { (format!("{} 失败", i.overview.failed), bad) } else { ("全部通过".to_string(), ok) };
                            h_flex()
                                .gap_3()
                                .items_center()
                                .child(
                                    Checkbox::new(("export-pick", ix))
                                        .label(i.dir.trim_start_matches("collections/").to_string())
                                        .checked(i.checked)
                                        .on_click(cx.listener(move |this, v: &bool, _, cx| {
                                            if let Some(it) = this.items.get_mut(ix) {
                                                it.checked = *v;
                                            }
                                            cx.notify();
                                        })),
                                )
                                .child(div().text_xs().text_color(muted).child(format!("{} 个用例", i.cases)))
                                .child(div().text_xs().text_color(state.1).child(state.0))
                        })),
                )
        });

        v_flex()
            .gap_4()
            .when_some(scope, |this, s| this.child(v_flex().gap_2().child(label("导出哪些结果")).child(s)))
            .when_some(list, |this, l| this.child(l))
            .child(div().text_sm().child(if self.use_run || picked > 0 { overview.headline() } else { "还没有勾选集合".to_string() }))
            .child(
                v_flex().gap_2().child(label("格式")).child(
                    RadioGroup::vertical("export-format")
                        .children([
                            "HTML 报告：每个用例的全部步骤与截图、失败现场和多端时间线都在里面，可翻页、放大到原始大小；单个文件，发给别人直接用浏览器打开",
                            "文本：结论 + 多端按先后排的时间线 + 失败详情，可直接贴进聊天、工单或给 AI",
                            "JSON：给程序处理",
                        ])
                        .selected_index(Some(match self.format {
                            Format::Html => 0,
                            Format::Text => 1,
                            Format::Json => 2,
                        }))
                        .on_change(cx.listener(|this, ix: &usize, _, cx| {
                            this.format = [Format::Html, Format::Text, Format::Json][(*ix).min(2)];
                            cx.notify();
                        })),
                ),
            )
            .when(!notes.is_empty(), |this| {
                this.child(v_flex().gap_1().children(notes.into_iter().map(|n| div().text_xs().text_color(cx.theme().warning).child(format!("· {n}")))))
            })
            .child(h_flex().gap_2().child(div().flex_1()).child(style::cli_button("export-cli", self.command())))
    }
}
