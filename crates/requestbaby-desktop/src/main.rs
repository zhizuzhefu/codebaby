//! requestbaby-desktop：给人用的观察与编辑界面。
//!
//! 与 CLI 共用 requestbaby-core；只读写项目目录与 `.requestbaby/runs/`，不引入私有状态。
mod case_editor;
mod config_editors;
mod dialogs;
mod project;
mod response;
mod run;
mod run_dialog;
mod shell;
mod vars_editor;
mod workspace;

use gpui_kit::component::{Theme, ThemeMode, TitleBar};
use gpui_kit::*;
use std::path::PathBuf;

actions!(
    requestbaby,
    [SaveCase, SendCase, RunAll, RunWithOptions, StopRun, ToggleTheme, OpenProject, NewProject, ImportSpec, Quit]
);

fn main() {
    let arg = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let arg = std::fs::canonicalize(&arg).unwrap_or(arg);
    // 不在项目内启动时展示欢迎页（打开 / 新建 / 导入）
    let root = requestbaby_core::session::project_root(&arg);

    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(move |cx| {
        gpui_kit::init(cx);
        if std::env::var("REQUESTBABY_THEME").is_ok_and(|t| t == "dark") {
            Theme::change(ThemeMode::Dark, None, cx);
        }
        cx.bind_keys([
            KeyBinding::new("cmd-s", SaveCase, None),
            KeyBinding::new("cmd-enter", SendCase, None),
            KeyBinding::new("cmd-r", RunAll, None),
            KeyBinding::new("cmd-shift-r", RunWithOptions, None),
            KeyBinding::new("cmd-o", OpenProject, None),
            KeyBinding::new("cmd-.", StopRun, None),
            KeyBinding::new("cmd-q", Quit, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.activate(true);

        let bounds = Bounds::centered(None, size(px(1480.), px(920.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(960.), px(600.))),
            ..TitleBar::window_options()
        };
        gpui_kit::open_window(options, cx, move |window, cx| {
            let view = cx.new(|cx| shell::Shell::new(root, window, cx));
            let focus = view.focus_handle(cx);
            window.defer(cx, move |window, cx| focus.focus(window, cx));
            view
        })
        .expect("failed to open window");
    });
}
