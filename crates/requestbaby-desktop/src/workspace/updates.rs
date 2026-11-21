//! 自动更新：后台检查（启动时与每 6 小时）→ 下载并完整校验 → 暂存在 App 旁边 → 退出时自动安装（或点「重启更新」立即装）。
//! 检查 / 下载 / 校验 / 替换走 core::update 与 core::update_app（与 `requestbaby upgrade` 同一实现）。
//! 任何一步失败都保留当前版本，原因写进日志（~/Library/Logs/requestbaby-update.log），下次启动告诉用户；
//! 不能自动更新时（没有写权限 / 从 DMG 里运行 / 开发构建）退回「下载 DMG 手动拖拽」。
use super::sidebar::menu_item;
use super::Workspace;
use crate::updates::{INTERVAL_MS, Prefs};
use gpui_kit::component::{
    Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    menu::{DropdownMenu as _, PopupMenuItem},
    notification::Notification,
};
use gpui_kit::{AnyElement, AppContext as _, Context, IntoElement, Window, prelude::FluentBuilder as _};
use requestbaby_core::update;
use requestbaby_core::update_app;
use std::time::Duration;

impl Workspace {
    /// 启动时：先处理上次更新的结果、恢复暂存好的新版本，再检查（开启了自动更新且距上次检查够久），之后每隔一段时间后台复查
    pub(super) fn auto_check_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.restore_update_state(window, cx);
        if Prefs::load().auto_due() {
            self.check_update(false, window, cx);
        }
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(INTERVAL_MS)).await;
                let alive = this.update_in(cx, |this, window, cx| {
                    if Prefs::load().auto_due() {
                        this.check_update(false, window, cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// 上次更新留下的结果（只提示一次）、待安装的暂存版本、过期的暂存与下载残留
    fn restore_update_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(o) = update_app::take_outcome() {
            let mut prefs = Prefs::load();
            if o.ok {
                prefs.failed_version = None;
                window.push_notification(Notification::success(format!("已更新到 {}", o.version)), cx);
            } else {
                prefs.failed_version = Some(o.version.clone());
                window.push_notification(Notification::error(format!("更新到 {} 失败，已保留当前版本 {}：{}", o.version, update::CURRENT, o.message)).autohide(false), cx);
            }
            prefs.save();
        }
        self.update_ready = update_app::read_pending();
        cx.background_spawn(async { update_app::cleanup_stale() }).detach();
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
                        let known_bad = !manual && prefs.failed_version.as_deref() == Some(rel.version.as_str());
                        let already = this.update_ready.as_ref().is_some_and(|p| p.version == rel.version);
                        this.update = Some(rel.clone());
                        if !already && !known_bad && (prefs.auto || manual) {
                            this.stage_update(rel, manual, window, cx);
                        } else if manual && already {
                            window.push_notification(Notification::info(format!("新版本 {} 已下载并校验：退出 requestbaby 时自动安装，或点状态栏「重启更新」", rel.version)), cx);
                        } else if manual {
                            window.push_notification(Notification::info(format!("有新版本 {}（当前 {}）：点状态栏右下角的「新版本」", rel.version, update::CURRENT)), cx);
                        }
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

    /// 后台下载并校验新版本，暂存在 App 旁边。不能自动更新时保持原来的「点状态栏手动下载 DMG」
    fn stage_update(&mut self, rel: update::Release, manual: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.update_staging {
            return;
        }
        let blocker = match update_app::current_app() {
            None => Some("当前是开发构建，不在 .app 里".to_string()),
            Some(app) => update_app::self_update_blocker(&app),
        };
        if let Some(why) = blocker {
            if manual {
                window.push_notification(Notification::info(format!("有新版本 {}，但不能自动更新：{why}。点状态栏「新版本」手动下载", rel.version)), cx);
            }
            return;
        }
        if manual {
            window.push_notification(Notification::info(format!("有新版本 {}（当前 {}）：正在后台下载并校验，完成后会提示", rel.version, update::CURRENT)), cx);
        }
        self.update_staging = true;
        let root = self.project.root().to_path_buf();
        let version = rel.version.clone();
        let task = cx.background_spawn(async move {
            let proxy = requestbaby_core::protocols::default_proxy(Some(&root))?;
            update_app::stage(&rel, &proxy)
        });
        cx.spawn_in(window, async move |this, cx| {
            let res = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.update_staging = false;
                match res {
                    Ok(p) => {
                        window.push_notification(
                            Notification::success(format!("新版本 {} 已下载并校验。退出 requestbaby 时会自动安装；想马上用，点右下角状态栏的「重启更新」，或菜单栏 requestbaby →「重启并更新…」", p.version)).autohide(false),
                            cx,
                        );
                        this.update_ready = Some(p);
                    }
                    // 自动准备失败：当前版本不受影响，状态栏仍可手动下载 DMG；后台检查时不打扰
                    Err(e) if manual => window.push_notification(Notification::error(format!("自动准备 {version} 失败，当前版本没有改动：{e:#}。可点状态栏「新版本」手动下载")), cx),
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// 立即重启并更新：把替换交给更新程序，本进程退出，替换完成后自动重新打开
    pub(super) fn restart_to_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.update_ready.is_none() {
            // 菜单栏「重启并更新」在没有待安装版本时：顺手检查一次，并说明情况
            window.push_notification(Notification::info("当前没有已下载好的新版本，正在检查更新…"), cx);
            self.check_update(true, window, cx);
            return;
        }
        let Some(p) = self.update_ready.clone().filter(|p| update_app::read_pending().as_ref() == Some(p)) else {
            self.update_ready = None;
            window.push_notification(Notification::error("暂存的新版本已失效，下次检查时会重新下载"), cx);
            cx.notify();
            return;
        };
        match update_app::spawn_apply(&p, true, 180) {
            Ok(()) => cx.quit(),
            Err(e) => window.push_notification(Notification::error(format!("无法开始更新，当前版本没有改动：{e:#}")), cx),
        }
    }

    /// 下载 DMG（校验 SHA256）到「下载」目录并打开：不能自动更新时，把 requestbaby 拖进「应用程序」即可替换旧版本
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

    /// 状态栏右侧：有新版本时的入口（已暂存 = 重启更新；准备中 = 进度；否则手动下载 DMG）
    pub(super) fn render_update_badge(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let view = cx.entity().downgrade();
        if let Some(p) = &self.update_ready {
            let (version, page) = (p.version.clone(), self.update.as_ref().map(|r| r.page.clone()).unwrap_or_default());
            let notes: Vec<String> = self.update.as_ref().map(|r| r.notes.lines().take(8).map(String::from).collect()).unwrap_or_default();
            return Some(
                Button::new("update-badge")
                    .xsmall()
                    .primary()
                    .rounded(crate::style::PILL)
                    .icon(Icon::new(IconName::RefreshCw))
                    .label(format!("重启更新 {version}"))
                    .tooltip("新版本已下载并校验，退出 requestbaby 时会自动安装")
                    .dropdown_menu(move |menu, _, _| {
                        let page = page.clone();
                        menu.item(menu_item(&view, format!("立即重启并更新到 {version}"), IconName::RefreshCw, |w, window, cx| w.restart_to_update(window, cx)))
                            .item(PopupMenuItem::label("不点也行：退出时会自动安装"))
                            .when(!page.is_empty(), |m| m.item(PopupMenuItem::new("查看更新说明").icon(IconName::ExternalLink).on_click(move |_, _, cx| cx.open_url(&page))))
                            .when(!notes.is_empty(), |m| notes.iter().fold(m.separator(), |m, l| m.item(PopupMenuItem::label(l.clone()))))
                    })
                    .into_any_element(),
            );
        }
        let rel = self.update.as_ref()?;
        if self.update_staging {
            return Some(Button::new("update-badge").xsmall().outline().rounded(crate::style::PILL).label(format!("正在准备 {}…", rel.version)).disabled(true).into_any_element());
        }
        let (version, page) = (rel.version.clone(), rel.page.clone());
        let notes: Vec<String> = rel.notes.lines().take(8).map(String::from).collect();
        Some(
            Button::new("update-badge")
                .xsmall()
                .primary()
                .rounded(crate::style::PILL)
                .icon(Icon::new(IconName::ArrowDown))
                .label(format!("新版本 {version}"))
                .tooltip("无法自动更新：下载 DMG 手动替换，或 requestbaby upgrade")
                .dropdown_menu(move |menu, _, _| {
                    let page = page.clone();
                    menu.item(menu_item(&view, format!("下载 {version}（DMG）"), IconName::ArrowDown, |w, window, cx| w.download_update(window, cx)))
                        .item(PopupMenuItem::new("查看更新说明").icon(IconName::ExternalLink).on_click(move |_, _, cx| cx.open_url(&page)))
                        .when(!notes.is_empty(), |m| notes.iter().fold(m.separator(), |m, l| m.item(PopupMenuItem::label(l.clone()))))
                })
                .into_any_element(),
        )
    }

    /// 项目菜单：已有下载好的新版本时，立即重启并更新
    pub(super) fn restart_update_item(view: &gpui_kit::WeakEntity<Workspace>, version: String) -> PopupMenuItem {
        menu_item(view, format!("重启并更新到 {version}"), IconName::RefreshCw, |w, window, cx| w.restart_to_update(window, cx))
    }

    /// 项目菜单：手动检查
    pub(super) fn check_update_item(view: &gpui_kit::WeakEntity<Workspace>) -> PopupMenuItem {
        menu_item(view, format!("检查更新…（当前 {}）", update::CURRENT), IconName::RefreshCw, |w, window, cx| w.check_update(true, window, cx))
    }

    /// 项目菜单：自动检查开关
    pub(super) fn auto_update_item(_view: &gpui_kit::WeakEntity<Workspace>) -> PopupMenuItem {
        let auto = Prefs::load().auto;
        PopupMenuItem::new("自动更新（后台下载，退出时安装）")
            .checked(auto)
            .on_click(move |_, window, cx| {
                let mut p = Prefs::load();
                p.auto = !p.auto;
                p.save();
                window.push_notification(Notification::info(if p.auto { "已开启：后台检查并下载新版本，退出时自动安装" } else { "已关闭自动更新（仍可手动检查）" }), cx);
            })
    }
}
