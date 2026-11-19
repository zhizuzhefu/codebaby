//! 新版本：启动时（每天一次）后台检查，状态栏提示；也可从项目菜单手动检查。
//! 检查 / 下载 / 校验走 core::update（与 `requestbaby upgrade` 同一实现）；桌面端下载签名公证过的 DMG 并打开。
use super::sidebar::menu_item;
use super::Workspace;
use crate::updates::Prefs;
use gpui_kit::component::{
    Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    menu::{DropdownMenu as _, PopupMenuItem},
    notification::Notification,
};
use gpui_kit::{AnyElement, AppContext as _, Context, IntoElement, Window, prelude::FluentBuilder as _};
use requestbaby_core::update;

impl Workspace {
    /// 启动时：开启了自动检查且距上次超过一天才检查（不打扰，没有新版本时不提示）
    pub(super) fn auto_check_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if Prefs::load().auto_due() {
            self.check_update(false, window, cx);
        }
    }

    /// manual = 手动检查：已是最新 / 失败也要告诉用户
    pub(super) fn check_update(&mut self, manual: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.update_busy {
            return;
        }
        self.update_busy = true;
        let root = self.project.root().to_path_buf();
        let task = cx.background_spawn(async move {
            let proxy = requestbaby_core::protocols::default_proxy(Some(&root))?;
            update::latest(&proxy)
        });
        cx.spawn_in(window, async move |this, cx| {
            let res = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.update_busy = false;
                let mut prefs = Prefs::load();
                prefs.last_check_ms = requestbaby_core::history::now_ms();
                prefs.save();
                match res {
                    Ok(Some(rel)) if rel.is_newer() => {
                        if manual {
                            window.push_notification(Notification::info(format!("有新版本 {}（当前 {}）：点状态栏右下角的「新版本」下载安装", rel.version, update::CURRENT)), cx);
                        }
                        this.update = Some(rel);
                    }
                    Ok(_) if manual => window.push_notification(Notification::success(format!("已是最新版本 {}", update::CURRENT)), cx),
                    Err(e) if manual => window.push_notification(Notification::error(format!("{e:#}")), cx),
                    _ => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 下载 DMG（校验 SHA256）到「下载」目录并打开：把 requestbaby 拖进「应用程序」即可替换旧版本
    fn download_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rel) = self.update.clone() else { return };
        let Some(asset) = rel.dmg_asset().cloned() else {
            cx.open_url(&rel.page);
            return;
        };
        window.push_notification(Notification::info(format!("正在下载 {}（{:.0} MB）…", asset.name, asset.size as f64 / 1_048_576.0)), cx);
        let root = self.project.root().to_path_buf();
        let task = cx.background_spawn(async move {
            let proxy = requestbaby_core::protocols::default_proxy(Some(&root))?;
            let dir = crate::response::downloads_dir();
            update::download(&rel, &asset, &dir, &proxy)
        });
        cx.spawn_in(window, async move |_, cx| {
            let res = task.await;
            let _ = cx.update(|window, cx| match res {
                Ok(path) => {
                    cx.open_url(&format!("file://{}", path.display()));
                    window.push_notification(Notification::success(format!("已下载并校验：{}。先退出本应用，再把 requestbaby 拖进「应用程序」替换旧版本", path.display())).autohide(false), cx);
                }
                Err(e) => window.push_notification(Notification::error(format!("下载失败：{e:#}")), cx),
            });
        })
        .detach();
    }

    /// 状态栏右侧：有新版本时的入口
    pub(super) fn render_update_badge(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let rel = self.update.as_ref()?;
        let view = cx.entity().downgrade();
        let (version, page) = (rel.version.clone(), rel.page.clone());
        let notes: Vec<String> = rel.notes.lines().take(8).map(String::from).collect();
        Some(
            Button::new("update-badge")
                .xsmall()
                .primary()
                .rounded(crate::style::PILL)
                .icon(Icon::new(IconName::ArrowDown))
                .label(format!("新版本 {version}"))
                .tooltip("requestbaby upgrade（命令行升级）")
                .dropdown_menu(move |menu, _, _| {
                    let page = page.clone();
                    menu.item(menu_item(&view, format!("下载并安装 {version}（DMG）"), IconName::ArrowDown, |w, window, cx| w.download_update(window, cx)))
                        .item(PopupMenuItem::new("查看更新说明").icon(IconName::ExternalLink).on_click(move |_, _, cx| cx.open_url(&page)))
                        .when(!notes.is_empty(), |m| notes.iter().fold(m.separator(), |m, l| m.item(PopupMenuItem::label(l.clone()))))
                })
                .into_any_element(),
        )
    }

    /// 项目菜单：手动检查
    pub(super) fn check_update_item(view: &gpui_kit::WeakEntity<Workspace>) -> PopupMenuItem {
        menu_item(view, format!("检查更新…（当前 {}）", update::CURRENT), IconName::RefreshCw, |w, window, cx| w.check_update(true, window, cx))
    }

    /// 项目菜单：自动检查开关
    pub(super) fn auto_update_item(_view: &gpui_kit::WeakEntity<Workspace>) -> PopupMenuItem {
        let auto = Prefs::load().auto;
        PopupMenuItem::new("启动时自动检查更新")
            .checked(auto)
            .on_click(move |_, window, cx| {
                let mut p = Prefs::load();
                p.auto = !p.auto;
                p.save();
                window.push_notification(Notification::info(if p.auto { "已开启：启动时每天最多检查一次" } else { "已关闭自动检查更新" }), cx);
            })
    }
}
