//! requestbaby-desktop：给人用的观察与编辑界面。
//!
//! 与 CLI 共用 requestbaby-core；只读写项目目录与 `.requestbaby/runs/`，不引入私有状态。
mod case_editor;
mod case_form;
mod cli_install;
mod config_editors;
mod dialogs;
mod new_case_dialog;
mod expect_table;
mod project;
mod recent;
mod response;
mod run;
mod run_dialog;
mod shell;
mod style;
mod updates;
mod vars_editor;
mod vars_panel;
mod kv_table;
mod workflow_editor;
mod workspace;

use gpui_kit::component::TitleBar;
use gpui_kit::*;
use std::path::PathBuf;

actions!(
    requestbaby,
    [SaveCase, SendCase, RunAll, RunWithOptions, Verify, StopRun, ToggleTheme, OpenProject, NewProject, ImportSpec, CloseProject, Quit, QuickOpen, FindInTree, InstallCli, ManageEnvs]
);

fn main() {
    let arg = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let arg = std::fs::canonicalize(&arg).unwrap_or(arg);
    // 不在项目内启动时展示欢迎页（打开 / 新建 / 导入）
    let root = requestbaby_core::session::project_root(&arg);

    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(move |cx| {
        gpui_kit::init(cx);
        style::install(cx);
        cx.bind_keys([
            KeyBinding::new("cmd-s", SaveCase, None),
            KeyBinding::new("cmd-p", QuickOpen, None),
            KeyBinding::new("cmd-shift-f", FindInTree, None),
            KeyBinding::new("cmd-enter", SendCase, None),
            KeyBinding::new("cmd-r", RunAll, None),
            KeyBinding::new("cmd-shift-r", RunWithOptions, None),
            KeyBinding::new("cmd-shift-v", Verify, None),
            KeyBinding::new("cmd-o", OpenProject, None),
            KeyBinding::new("cmd-w", CloseProject, None),
            KeyBinding::new("cmd-.", StopRun, None),
            KeyBinding::new("cmd-q", Quit, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        // 单窗口应用：窗口关闭即退出，不留下没有窗口的 Dock 图标
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.set_menus([
            Menu::new("requestbaby").items([
                MenuItem::action("安装命令行工具…", InstallCli),
                MenuItem::separator(),
                MenuItem::action("退出 requestbaby", Quit),
            ]),
            Menu::new("文件").items([
                MenuItem::action("打开项目…", OpenProject),
                MenuItem::action("新建项目…", NewProject),
                MenuItem::action("从 OpenAPI / Postman 导入…", ImportSpec),
                MenuItem::separator(),
                MenuItem::action("保存", SaveCase),
                MenuItem::separator(),
                MenuItem::action("关闭项目", CloseProject),
            ]),
            Menu::new("运行").items([
                MenuItem::action("完整验证", Verify),
                MenuItem::action("运行全部", RunAll),
                MenuItem::action("运行配置…", RunWithOptions),
                MenuItem::action("发送当前用例", SendCase),
                MenuItem::separator(),
                MenuItem::action("停止", StopRun),
            ]),
            Menu::new("视图").items([
                MenuItem::action("快速打开…", QuickOpen),
                MenuItem::action("在集合中筛选", FindInTree),
                MenuItem::action("环境管理…", ManageEnvs),
                MenuItem::separator(),
                MenuItem::action("切换深色 / 浅色主题", ToggleTheme),
            ]),
        ]);
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
