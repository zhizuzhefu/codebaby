//! 窗口外壳：持有当前项目的工作区；打开 / 新建 / 导入 / 关闭项目（core::scaffold / core::importer）。
use crate::dialogs::confirm_discard;
use crate::workspace::Workspace;
use crate::recent;
use crate::{CloseProject, ImportSpec, InstallCli, NewProject, OpenProject, ToggleTheme};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, Theme, ThemeMode, TitleBar, WindowExt as _, h_flex,
    button::Button,
    notification::Notification,
    v_flex,
};
use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, PathPromptOptions, Render, StatefulInteractiveElement as _, Styled as _, Subscription, Window,
    div, prelude::FluentBuilder as _, px,
};
use requestbaby_core::{importer, scaffold, session};
use std::path::PathBuf;

/// 工作区请求外壳处理的操作（会替换当前工作区）
pub enum ShellRequest {
    OpenProject,
    NewProject,
    ImportSpec,
    CloseProject,
}

pub struct Shell {
    workspace: Option<Entity<Workspace>>,
    focus_handle: FocusHandle,
    _subscription: Option<Subscription>,
}

impl Focusable for Shell {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Shell {
    /// `root` 为 None 时展示欢迎页（启动目录不是 requestbaby 项目）
    pub fn new(root: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self { workspace: None, focus_handle: cx.focus_handle(), _subscription: None };
        match root {
            Some(root) => this.open_project(root, window, cx),
            None => window.set_window_title("requestbaby"),
        }
        this
    }

    fn open_project(&mut self, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        window.set_window_title(&format!("requestbaby — {}", root.display()));
        recent::remember(&root);
        let ws = cx.new(|cx| Workspace::new(root, window, cx));
        self._subscription = Some(cx.subscribe_in(&ws, window, |this, _, req: &ShellRequest, window, cx| match req {
            ShellRequest::OpenProject => this.request(Self::prompt_open, window, cx),
            ShellRequest::NewProject => this.request(Self::prompt_new, window, cx),
            ShellRequest::ImportSpec => this.request(Self::prompt_import, window, cx),
            ShellRequest::CloseProject => this.request(Self::close_project, window, cx),
        }));
        let focus = ws.focus_handle(cx);
        window.defer(cx, move |window, cx| focus.focus(window, cx));
        self.workspace = Some(ws);
        cx.notify();
    }

    /// 切换项目前确认未保存的修改
    fn request(&mut self, f: fn(&mut Self, &mut Window, &mut Context<Self>), window: &mut Window, cx: &mut Context<Self>) {
        let lost = self.workspace.as_ref().map(|w| w.read(cx).dirty_documents(cx)).unwrap_or_default();
        if lost.is_empty() {
            f(self, window, cx);
            return;
        }
        let this = cx.entity().downgrade();
        confirm_discard(lost, window, cx, move |window, cx| {
            let _ = this.update(cx, |s, cx| f(s, window, cx));
        });
    }

    /// 回到欢迎页；没有打开项目时 ⌘W 关闭窗口（随即退出应用）
    fn close_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.is_none() {
            window.remove_window();
            return;
        }
        self.workspace = None;
        self._subscription = None;
        window.set_window_title("requestbaby");
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    /// 安装命令行工具：后台执行（可能弹出系统授权对话框），结果以通知展示
    fn install_cli(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let task = cx.background_spawn(async move { crate::cli_install::install() });
        cx.spawn_in(window, async move |_, cx| {
            let res = task.await;
            let _ = cx.update(|window, cx| match res {
                Ok(done) => {
                    window.push_notification(Notification::success(done.message), cx);
                    if let Some(w) = done.warning {
                        window.push_notification(Notification::warning(w).autohide(false), cx);
                    }
                }
                Err(e) => window.push_notification(Notification::error(format!("安装命令行工具失败：{e:#}")), cx),
            });
        })
        .detach();
    }

    fn prompt_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("打开项目".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(dir) = paths.into_iter().next() else { return };
            let _ = this.update_in(cx, |s, window, cx| match session::project_root(&dir) {
                Some(root) => s.open_project(root, window, cx),
                None => window.push_notification(
                    Notification::warning(format!("{} 不是 requestbaby 项目（未找到 requestbaby.json）：可用「新建项目」初始化", dir.display())),
                    cx,
                ),
            });
        })
        .detach();
    }

    fn prompt_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("在此初始化".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(dir) = paths.into_iter().next() else { return };
            let _ = this.update_in(cx, |s, window, cx| match scaffold::init_project(&dir) {
                Ok(entries) => {
                    let created = entries.iter().filter(|e| e.created).count();
                    window.push_notification(
                        Notification::success(format!("已初始化 {}：新建 {created} 个文件", dir.display())),
                        cx,
                    );
                    s.open_project(dir, window, cx);
                }
                Err(e) => window.push_notification(Notification::error(format!("{e:#}")), cx),
            });
        })
        .detach();
    }

    /// 选择 OpenAPI / Swagger / Postman 文件 → 选择新项目目录 → 导入并打开
    fn prompt_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("导入".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(spec) = paths.into_iter().next() else { return };
            let parent = spec.parent().map(PathBuf::from).unwrap_or_default();
            let stem = spec.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "imported".into());
            let Ok(rx) = cx.update(|_, cx| cx.prompt_for_new_path(&parent, Some(&stem))) else { return };
            let Ok(Ok(Some(out))) = rx.await else { return };
            let _ = this.update_in(cx, |s, window, cx| match importer::import(&spec, &out, "dev") {
                Ok(sum) => {
                    let mut text = format!("导入完成：{} 个用例 / {} 个集合（baseUrl {}）", sum.cases, sum.collections, sum.base_url);
                    if !sum.warnings.is_empty() {
                        text.push_str(&format!("，{} 条提示", sum.warnings.len()));
                    }
                    window.push_notification(Notification::success(text), cx);
                    s.open_project(out, window, cx);
                }
                Err(e) => window.push_notification(Notification::error(format!("导入失败：{e:#}")), cx),
            });
        })
        .detach();
    }

    fn render_welcome(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let action = |id: &'static str, icon: IconName, label: &'static str, hint: &'static str, f: fn(&mut Self, &mut Window, &mut Context<Self>)| {
            v_flex()
                .w(px(220.))
                .p_4()
                .gap_2()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(cx.theme().border)
                .child(Icon::new(icon).text_color(cx.theme().primary))
                .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(label))
                .child(div().flex_1().text_xs().text_color(muted).child(hint))
                .child(Button::new(id).small().outline().label(label).on_click(cx.listener(move |this, _, window, cx| this.request(f, window, cx))))
        };
        v_flex()
            .size_full()
            .child(TitleBar::new().child(
                h_flex()
                    .gap_2()
                    .child(Icon::new(IconName::Bot).small().text_color(cx.theme().primary))
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("requestbaby")),
            ))
            .child(
                v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap_6()
                    .child(
                        v_flex()
                            .items_center()
                            .gap_1()
                            .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child("AI 交付的独立验收层"))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(muted)
                                    .child("辅助人验收 AI 交付的成果 · CLI、AI 与桌面端读写同一份文件，运行历史共享于 .requestbaby/runs/"),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_4()
                            .items_stretch()
                            .child(action("welcome-open", IconName::FolderOpen, "打开项目", "选择包含 requestbaby.json 的目录（或其子目录）", Self::prompt_open))
                            .child(action("welcome-new", IconName::Plus, "新建项目", "生成项目配置、dev / prod 环境与示例用例", Self::prompt_new))
                            .child(action("welcome-import", IconName::Inbox, "导入", "从 OpenAPI / Swagger / Postman 生成完整项目", Self::prompt_import)),
                    )
                    .child(self.render_recent(cx))
                    .child(div().text_xs().text_color(muted).child("也可在终端执行 requestbaby open 直接打开当前项目")),
            )
    }
}

impl Shell {
    fn render_recent(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let recent = recent::load();
        let muted = cx.theme().muted_foreground;
        v_flex()
            .w(px(692.))
            .gap_1()
            .when(!recent.is_empty(), |this| {
                this.child(div().text_xs().font_weight(FontWeight::MEDIUM).text_color(muted).child("最近打开"))
            })
            .children(recent.into_iter().enumerate().map(|(ix, root)| {
                let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let path = root.display().to_string();
                h_flex()
                    .id(("recent", ix))
                    .px_2()
                    .py_1()
                    .gap_3()
                    .rounded(cx.theme().radius)
                    .cursor_pointer()
                    .hover(|d| d.bg(cx.theme().table_hover))
                    .on_click(cx.listener(move |this, _, window, cx| this.open_project(root.clone(), window, cx)))
                    .child(Icon::new(IconName::Folder).small().text_color(muted))
                    .child(div().w(px(160.)).text_sm().font_weight(FontWeight::MEDIUM).child(name))
                    .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().text_xs().text_color(muted).child(path))
            }))
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = window;
        let root = div()
            .id("shell")
            .key_context("Shell")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(|this, _: &OpenProject, window, cx| this.request(Self::prompt_open, window, cx)))
            .on_action(cx.listener(|this, _: &NewProject, window, cx| this.request(Self::prompt_new, window, cx)))
            .on_action(cx.listener(|this, _: &ImportSpec, window, cx| this.request(Self::prompt_import, window, cx)))
            .on_action(cx.listener(|this, _: &CloseProject, window, cx| this.request(Self::close_project, window, cx)))
            .on_action(cx.listener(|this, _: &InstallCli, window, cx| this.install_cli(window, cx)))
            .on_action(cx.listener(|_, _: &ToggleTheme, window, cx| {
                let mode = if cx.theme().mode.is_dark() { ThemeMode::Light } else { ThemeMode::Dark };
                Theme::change(mode, Some(window), cx);
            }));
        match &self.workspace {
            Some(ws) => root.child(ws.clone()),
            None => root.child(self.render_welcome(cx)),
        }
    }
}
