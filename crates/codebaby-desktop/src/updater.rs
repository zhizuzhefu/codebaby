//! 自动更新：后台检查（启动时与每 6 小时）→ 下载并完整校验 → 暂存在 App 旁边 → 退出时自动安装（或点「重启更新」立即装）。
//! 检查 / 下载 / 校验 / 替换走 core::update 与 core::update_app（与 `codebaby upgrade` 同一实现）。
//! 任何一步失败都保留当前版本，原因写进日志（~/Library/Logs/codebaby-update.log），下次启动告诉用户；
//! 不能自动更新时（没有写权限 / 从 DMG 里运行 / 开发构建）退回「下载 DMG 手动拖拽」。
use crate::updates::{INTERVAL_MS, Prefs};
use gpui_kit::component::{
    Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    menu::{DropdownMenu as _, PopupMenuItem},
    notification::Notification,
};
use gpui_kit::{AnyElement, AppContext as _, Context, IntoElement, WeakEntity, Window, prelude::FluentBuilder as _};
use codebaby_core::update;
use codebaby_core::update_app;
use std::path::PathBuf;
use std::time::Duration;

/// 应用级的更新状态：不依赖打开的项目（欢迎页也会检查），由窗口外壳持有，工作区只负责展示入口
#[derive(Default)]
pub struct Updater {
    /// 当前项目目录：只用来取代理设置，没有打开项目时为 None
    pub root: Option<PathBuf>,
    /// 检查到的新版本（状态栏提示）
    update: Option<update::Release>,
    busy: bool,
    /// 已下载并校验、暂存好待安装的新版本（退出时自动安装）
    ready: Option<update_app::Pending>,
    /// 正在后台下载 / 校验新版本
    staging: bool,
}

/// 弹出菜单项：对更新状态执行一个操作
fn item(view: &WeakEntity<Updater>, label: impl Into<gpui_kit::SharedString>, icon: IconName, f: impl Fn(&mut Updater, &mut Window, &mut Context<Updater>) + 'static) -> PopupMenuItem {
    let view = view.clone();
    PopupMenuItem::new(label).icon(icon).on_click(move |_, window, cx| {
        let _ = view.update(cx, |u, cx| f(u, window, cx));
    })
}

impl Updater {
    /// 启动时：先处理上次更新的结果、恢复暂存好的新版本，再检查（开启了自动更新且距上次检查够久），之后每隔一段时间后台复查
    pub fn auto_check(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.restore_update_state(window, cx);
        if Prefs::load().auto_due() {
            self.check(false, window, cx);
        }
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(INTERVAL_MS)).await;
                let alive = this.update_in(cx, |this, window, cx| {
                    if Prefs::load().auto_due() {
                        this.check(false, window, cx);
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
        self.ready = update_app::read_pending();
        cx.background_spawn(async { update_app::cleanup_stale() }).detach();
        // 状态变了要通知界面（欢迎页 / 状态栏的入口）刷新
        cx.notify();
    }

    /// manual = 手动检查：已是最新 / 失败也要告诉用户
    pub fn check(&mut self, manual: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        let root = self.root.clone();
        let task = cx.background_spawn(async move {
            let proxy = codebaby_core::protocols::default_proxy(root.as_deref())?;
            update::latest(&proxy)
        });
        cx.spawn_in(window, async move |this, cx| {
            let res = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                let mut prefs = Prefs::load();
                prefs.last_check_ms = codebaby_core::history::now_ms();
                prefs.save();
                match res {
                    Ok(Some(rel)) if rel.is_newer() => {
                        let known_bad = !manual && prefs.failed_version.as_deref() == Some(rel.version.as_str());
                        let already = this.ready.as_ref().is_some_and(|p| p.version == rel.version);
                        this.update = Some(rel.clone());
                        // 自动更新等新版本发布满一天再下载（连发几个补丁版只装最后一个）；手动检查立即生效
                        let settled = rel.settled(codebaby_core::history::now_ms() / 1000);
                        if !already && !known_bad && (manual || (prefs.auto && settled)) {
                            this.stage_update(rel, manual, window, cx);
                        } else if manual && already {
                            this.prompt_install(rel.version.clone(), window, cx);
                        } else if manual {
                            window.push_notification(Notification::info(format!("有新版本 {}（当前 {}），但没有自动更新：点状态栏右下角的「新版本」下载", rel.version, update::CURRENT)), cx);
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
        if self.staging {
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
            window.push_notification(Notification::info(format!("发现新版本 {}（当前 {}），正在下载，完成后点「安装并更新」即可", rel.version, update::CURRENT)), cx);
        }
        self.staging = true;
        let root = self.root.clone();
        let version = rel.version.clone();
        let task = cx.background_spawn(async move {
            let proxy = codebaby_core::protocols::default_proxy(root.as_deref())?;
            update_app::stage(&rel, &proxy)
        });
        cx.spawn_in(window, async move |this, cx| {
            let res = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.staging = false;
                match res {
                    Ok(p) => {
                        let version = p.version.clone();
                        this.ready = Some(p);
                        if manual {
                            this.prompt_install(version, window, cx);
                        } else {
                            window.push_notification(Notification::info(format!("新版本 {version} 已下载，点状态栏的「安装并更新」即可")), cx);
                        }
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

    /// 新版本已下载并校验：只有一个选项「安装并更新」（按 Esc 关掉也不要紧，退出时会自动安装）
    fn prompt_install(&mut self, version: String, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let view = view.clone();
            alert.title(format!("新版本 {version} 已下载")).description("安装后 codebaby 会自动重启并打开。").ok_text("安装并更新").on_ok(move |_, window, cx| {
                let _ = view.update(cx, |u, cx| u.restart_to_update(window, cx));
                true
            })
        });
    }

    /// 立即重启并更新：把替换交给更新程序，本进程退出，替换完成后自动重新打开
    pub fn restart_to_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.ready.clone().filter(|p| update_app::read_pending().as_ref() == Some(p)) else {
            self.ready = None;
            window.push_notification(Notification::error("暂存的新版本已失效，下次检查时会重新下载"), cx);
            cx.notify();
            return;
        };
        match update_app::spawn_apply(&p, true, 180) {
            Ok(()) => cx.quit(),
            Err(e) => window.push_notification(Notification::error(format!("无法开始更新，当前版本没有改动：{e:#}")), cx),
        }
    }

    /// 下载 DMG（校验 SHA256）到「下载」目录并打开：不能自动更新时，把 codebaby 拖进「应用程序」即可替换旧版本
    pub fn download_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rel) = self.update.clone() else { return };
        let Some(asset) = rel.dmg_asset().cloned() else {
            cx.open_url(&rel.page);
            return;
        };
        window.push_notification(Notification::info(format!("正在下载 {}（{:.0} MB）…", asset.name, asset.size as f64 / 1_048_576.0)), cx);
        let root = self.root.clone();
        let task = cx.background_spawn(async move {
            let proxy = codebaby_core::protocols::default_proxy(root.as_deref())?;
            let dir = crate::response::downloads_dir();
            update::download(&rel, &asset, &dir, &proxy)
        });
        cx.spawn_in(window, async move |_, cx| {
            let res = task.await;
            let _ = cx.update(|window, cx| match res {
                Ok(path) => {
                    cx.open_url(&format!("file://{}", path.display()));
                    window.push_notification(Notification::success(format!("已下载并校验：{}。先退出本应用，再把 codebaby 拖进「应用程序」替换旧版本", path.display())).autohide(false), cx);
                }
                Err(e) => window.push_notification(Notification::error(format!("下载失败：{e:#}")), cx),
            });
        })
        .detach();
    }

    /// 状态栏右侧：有新版本时的入口（已暂存 = 安装并更新，点一下就装；准备中 = 进度；否则手动下载 DMG）
    pub fn render_badge(&self, view: WeakEntity<Updater>) -> Option<AnyElement> {
        if let Some(p) = &self.ready {
            let version = p.version.clone();
            return Some(
                Button::new("update-badge")
                    .xsmall()
                    .primary()
                    .rounded(crate::style::PILL)
                    .icon(Icon::new(IconName::RefreshCw))
                    .label(format!("安装并更新 {version}"))
                    .tooltip("新版本已下载并校验：点击后重启并安装，完成后自动重新打开")
                    .on_click(move |_, window, cx| {
                        let _ = view.update(cx, |u, cx| u.restart_to_update(window, cx));
                    })
                    .into_any_element(),
            );
        }
        let rel = self.update.as_ref()?;
        if self.staging {
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
                .tooltip("无法自动更新：下载 DMG 手动替换，或 codebaby upgrade")
                .dropdown_menu(move |menu, _, _| {
                    let page = page.clone();
                    menu.item(item(&view, format!("下载 {version}（DMG）"), IconName::ArrowDown, |w, window, cx| w.download_update(window, cx)))
                        .item(PopupMenuItem::new("查看更新说明").icon(IconName::ExternalLink).on_click(move |_, _, cx| cx.open_url(&page)))
                        .when(!notes.is_empty(), |m| notes.iter().fold(m.separator(), |m, l| m.item(PopupMenuItem::label(l.clone()))))
                })
                .into_any_element(),
        )
    }

    /// 已暂存、待安装的新版本号
    pub fn ready_version(&self) -> Option<String> {
        self.ready.as_ref().map(|p| p.version.clone())
    }

    /// 项目菜单：已有下载好的新版本时，立即重启并更新
    pub fn restart_item(view: &WeakEntity<Updater>, version: String) -> PopupMenuItem {
        item(view, format!("安装并更新到 {version}"), IconName::RefreshCw, |u, window, cx| u.restart_to_update(window, cx))
    }

    /// 项目菜单：手动检查
    pub fn check_item(view: &WeakEntity<Updater>) -> PopupMenuItem {
        item(view, format!("检查更新…（当前 {}）", update::CURRENT), IconName::RefreshCw, |u, window, cx| u.check(true, window, cx))
    }

    /// 项目菜单：自动检查开关
    pub fn auto_item() -> PopupMenuItem {
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
