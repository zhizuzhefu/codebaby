//! codebaby-desktop：给人用的观察与编辑界面。
//!
//! 与 CLI 共用 codebaby-core；只读写项目目录与 `.codebaby/runs/`，不引入私有状态。
mod case_editor;
mod case_form;
mod cli_install;
mod config_editors;
mod dialogs;
mod new_case_dialog;
mod expect_table;
mod export_dialog;
mod project;
mod recent;
mod response;
mod run;
mod run_dialog;
mod shell;
mod step_viewer;
mod style;
mod updater;
mod updates;
mod vars_editor;
mod vars_panel;
mod kv_table;
mod workflow_editor;
mod workspace;

use gpui_kit::component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
use gpui_kit::component::TitleBar;
use gpui_kit::*;
use std::path::PathBuf;

actions!(
    codebaby,
    [SaveCase, SendCase, RunAll, RunWithOptions, Verify, StopRun, ToggleTheme, OpenProject, NewProject, ImportSpec, CloseProject, CopyProjectPath, ExportResults, Quit, QuickOpen, FindInTree, InstallCli, ManageEnvs, OpenWebsite, OpenDocs, OpenSource, ReportIssue, CheckUpdate, About, HideApp, HideOthers, ShowAll, Minimize, Zoom, ToggleFullscreen]
);

fn main() {
    let arg = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let arg = std::fs::canonicalize(&arg).unwrap_or(arg);
    // 不在项目内启动时展示欢迎页（打开 / 新建 / 导入）
    let root = codebaby_core::session::project_root(&arg);

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
            KeyBinding::new("cmd-alt-c", CopyProjectPath, None),
            KeyBinding::new("cmd-e", ExportResults, None),
            KeyBinding::new("cmd-.", StopRun, None),
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-h", HideApp, None),
            KeyBinding::new("cmd-alt-h", HideOthers, None),
            KeyBinding::new("cmd-m", Minimize, None),
            KeyBinding::new("ctrl-cmd-f", ToggleFullscreen, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        // 关于 / 帮助：不依赖当前打开的项目，欢迎页与工作区都可用
        cx.on_action(|_: &OpenWebsite, cx| cx.open_url(codebaby_core::about::WEBSITE));
        cx.on_action(|_: &OpenDocs, cx| cx.open_url(codebaby_core::about::DOCS));
        cx.on_action(|_: &OpenSource, cx| cx.open_url(codebaby_core::about::SOURCE));
        cx.on_action(|_: &ReportIssue, cx| cx.open_url(codebaby_core::about::ISSUES));
        // 应用与窗口的系统级操作（与 macOS 标准菜单一致）
        cx.on_action(|_: &HideApp, cx| cx.hide());
        cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
        cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
        cx.on_action(|_: &Minimize, cx| window_op(cx, |w| w.minimize_window()));
        cx.on_action(|_: &Zoom, cx| window_op(cx, |w| w.zoom_window()));
        cx.on_action(|_: &ToggleFullscreen, cx| window_op(cx, |w| w.toggle_fullscreen()));
        // 已下载并校验好新版本时，退出就是安装时机：把替换交给独立的更新程序（它等本进程退出后原子替换，失败保留旧版本）
        cx.on_app_quit(|_| {
            if let Some(p) = codebaby_core::update_app::read_pending() {
                let _ = codebaby_core::update_app::spawn_apply(&p, false, 180);
            }
            async {}
        })
        .detach();
        // 单窗口应用：窗口关闭即退出，不留下没有窗口的 Dock 图标
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.set_menus([
            Menu::new("codebaby").items([
                MenuItem::action("关于 codebaby", About),
                MenuItem::action("检查更新…", CheckUpdate),
                MenuItem::separator(),
                MenuItem::action("安装命令行工具…", InstallCli),
                MenuItem::separator(),
                MenuItem::os_submenu("服务", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("隐藏 codebaby", HideApp),
                MenuItem::action("隐藏其他", HideOthers),
                MenuItem::action("全部显示", ShowAll),
                MenuItem::separator(),
                MenuItem::action("退出 codebaby", Quit),
            ]),
            Menu::new("文件").items([
                MenuItem::action("打开项目…", OpenProject),
                MenuItem::action("新建项目…", NewProject),
                MenuItem::action("从 OpenAPI / Postman 导入…", ImportSpec),
                MenuItem::separator(),
                MenuItem::action("保存", SaveCase),
                MenuItem::separator(),
                MenuItem::action("导出结果…", ExportResults),
                MenuItem::separator(),
                MenuItem::action("复制项目路径", CopyProjectPath),
                MenuItem::action("关闭项目", CloseProject),
            ]),
            Menu::new("编辑").items([
                MenuItem::os_action("撤销", Undo, OsAction::Undo),
                MenuItem::os_action("重做", Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("剪切", Cut, OsAction::Cut),
                MenuItem::os_action("拷贝", Copy, OsAction::Copy),
                MenuItem::os_action("粘贴", Paste, OsAction::Paste),
                MenuItem::os_action("全选", SelectAll, OsAction::SelectAll),
                MenuItem::separator(),
                MenuItem::action("在集合中筛选", FindInTree),
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
                MenuItem::action("环境管理…", ManageEnvs),
                MenuItem::separator(),
                MenuItem::action("切换深色 / 浅色主题", ToggleTheme),
                MenuItem::action("进入 / 退出全屏", ToggleFullscreen),
            ]),
            Menu::new("窗口").items([
                MenuItem::action("最小化", Minimize),
                MenuItem::action("缩放", Zoom),
            ]),
            Menu::new("帮助").items([
                MenuItem::action(format!("关于 codebaby · 版本 {}", codebaby_core::update::CURRENT), About),
                MenuItem::action("检查更新…", CheckUpdate),
                MenuItem::separator(),
                MenuItem::action("官网与下载", OpenWebsite),
                MenuItem::action("文档", OpenDocs),
                MenuItem::action("源码仓库", OpenSource),
                MenuItem::separator(),
                MenuItem::action("反馈问题…", ReportIssue),
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

/// 对当前活动窗口做一个窗口操作（最小化 / 缩放 / 全屏）：菜单是应用级的，没有窗口上下文
fn window_op(cx: &mut App, f: impl FnOnce(&mut Window) + 'static) {
    if let Some(handle) = cx.active_window() {
        let _ = handle.update(cx, |_, window, _| f(window));
    }
}
