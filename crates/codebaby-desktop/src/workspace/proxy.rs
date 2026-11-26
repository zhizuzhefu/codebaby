//! 「⋯」菜单里的代理入口：一键切换 跟随系统 / 使用代理 / 不使用代理（直连），地址与直连主机在「项目设置」里改。
//! 读写走 core::proxy_setting，与 `codebaby proxy on / off / unset` 和项目设置里的「代理」同一实现。
use super::sidebar::menu_item;
use super::{Target, Workspace};
use gpui_kit::component::{
    IconName, WindowExt as _,
    menu::{PopupMenu, PopupMenuItem},
    notification::Notification,
};
use gpui_kit::{Context, WeakEntity, Window};
use codebaby_core::proxy_setting::{self, Mode};

impl Workspace {
    pub(super) fn proxy_mode(&self) -> Mode {
        Mode::of(self.project.config().proxy.as_ref())
    }

    /// 切换模式：地址 / 直连主机沿用已有设置；要「使用代理」但还没有地址时转到项目设置去填
    fn set_proxy_mode(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        let cur = self.project.config().proxy.clone();
        let (url, no_proxy) = cur.map(|c| (c.url, c.no_proxy)).unwrap_or_default();
        if mode == Mode::On && url.trim().is_empty() {
            window.push_notification(Notification::info("先在「项目设置 → 代理」填写代理地址（socks5://主机:端口）"), cx);
            self.open(Target::Project, window, cx);
            return;
        }
        let res = proxy_setting::build(mode, &url, no_proxy).and_then(|cfg| proxy_setting::write(self.project.root(), cfg.as_ref()));
        match res {
            Ok(()) => {
                self.reload_files(window, cx);
                window.push_notification(Notification::success(format!("代理：{}", mode.label())), cx);
            }
            Err(e) => window.push_notification(Notification::error(format!("{e:#}")), cx),
        }
    }

    /// 「⋯」菜单里的代理模式（勾选当前模式）与代理设置入口
    pub(super) fn proxy_menu_items(menu: PopupMenu, view: &WeakEntity<Workspace>, mode: Mode) -> PopupMenu {
        let mut menu = menu.separator().item(PopupMenuItem::label("网络代理"));
        for m in [Mode::System, Mode::On, Mode::Off] {
            let view = view.clone();
            menu = menu.item(PopupMenuItem::new(m.label()).checked(m == mode).on_click(move |_, window, cx| {
                let _ = view.update(cx, |w, cx| w.set_proxy_mode(m, window, cx));
            }));
        }
        menu.item(menu_item(view, "代理设置…", IconName::Settings2, |w, window, cx| w.open(Target::Project, window, cx)))
    }
}
