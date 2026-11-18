//! 环境管理页：全部环境一览，切换 / 编辑变量 / 设为默认 / 新建 / 重命名 / 复制 / 删除。
//! 增删改全部走 core::envs，与 `requestbaby env …` 同一实现（重命名同步 defaultEnv 与固定的流程，删除连同本机覆盖）。
use super::sidebar::menu_item;
use super::{Center, Target, Workspace};
use crate::dialogs;
use crate::style;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex,
    button::{Button, ButtonVariants as _},
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
    notification::Notification,
    tag::Tag,
    v_flex,
};
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _,
    Styled as _, WeakEntity, Window, div, prelude::FluentBuilder as _,
};
use requestbaby_core::envs;
use requestbaby_core::varstore::VarScope;

impl Workspace {
    pub(super) fn render_env_manager(&self, cx: &mut Context<Self>) -> AnyElement {
        let list = envs::list(self.project.root());
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let view = cx.entity().downgrade();
        let header = h_flex()
            .gap_3()
            .items_start()
            .child(
                v_flex()
                    .flex_1()
                    .gap_1()
                    .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("环境"))
                    .child(div().text_xs().text_color(muted).child(
                        "每个环境是一个 envs/<名称>.json；运行时用标题栏选中的环境。不带 -e 的命令行运行使用默认环境。本机覆盖 envs/<名称>.local.json 放密钥与本机端口，不入库。",
                    )),
            )
            .child(style::cli_button(
                "envs-cli",
                "requestbaby env list · env new <名称> [--from 环境] · env rename <旧> <新> · env cp <源> <新> · env rm <名称> · env default <名称>",
            ))
            .child(
                Button::new("env-new")
                    .small()
                    .primary()
                    .rounded(style::PILL)
                    .icon(IconName::Plus)
                    .label("新建环境")
                    .on_click(cx.listener(|this, _, window, cx| this.new_env(None, window, cx))),
            );
        let body: AnyElement = match list {
            Err(e) => div().text_sm().text_color(theme.danger).child(format!("{e:#}")).into_any_element(),
            Ok(list) if list.is_empty() => div()
                .p_6()
                .rounded(theme.radius_lg)
                .border_1()
                .border_dashed()
                .border_color(border)
                .text_sm()
                .text_color(muted)
                .child("还没有环境：点「新建环境」")
                .into_any_element(),
            Ok(list) => {
                let count = list.len();
                v_flex()
                    .rounded(theme.radius_lg)
                    .border_1()
                    .border_color(border)
                    .overflow_hidden()
                    .children(list.into_iter().enumerate().map(|(ix, e)| {
                        let current = e.name == self.env;
                        let (n1, n2, n3) = (e.name.clone(), e.name.clone(), e.name.clone());
                        let (is_default, view) = (e.is_default, view.clone());
                        let mut meta = vec![format!("{} 个变量", e.vars)];
                        if let Some(n) = e.local_vars {
                            meta.push(format!("本机覆盖 {n} 个"));
                        }
                        if !e.pinned_by.is_empty() {
                            meta.push(format!("固定的流程：{}", e.pinned_by.join("、")));
                        }
                        h_flex()
                            .id(("env-row", ix))
                            .px_4()
                            .py_3()
                            .gap_3()
                            .when(ix + 1 < count, |row| row.border_b_1().border_color(border))
                            .when(current, |row| row.bg(cx.theme().table_active))
                            .child(Icon::new(IconName::Globe).small().text_color(if current { cx.theme().primary } else { muted }))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(e.name.clone()))
                                            .when(current, |h| h.child(Tag::primary().xsmall().child("当前")))
                                            .when(e.is_default, |h| h.child(Tag::success().outline().xsmall().child("默认")))
                                            .when(e.error.is_some(), |h| h.child(Tag::danger().outline().xsmall().child("文件有误"))),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .text_color(if e.base_url.is_some() { cx.theme().foreground } else { muted })
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .child(e.base_url.clone().map(|b| format!("baseUrl = {b}")).unwrap_or_else(|| "未设置 baseUrl".into())),
                                    )
                                    .child(div().text_xs().text_color(muted).child(e.error.clone().unwrap_or_else(|| meta.join(" · ")))),
                            )
                            .when(!current, |row| {
                                row.child(
                                    Button::new(("env-use", ix))
                                        .small()
                                        .outline()
                                        .label("切换到此环境")
                                        .on_click(cx.listener(move |this, _, window, cx| this.switch_env(n1.clone(), window, cx))),
                                )
                            })
                            .when(current, |row| {
                                row.child(
                                    Button::new(("env-vars", ix))
                                        .small()
                                        .ghost()
                                        .icon(IconName::Settings2)
                                        .label("编辑变量")
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.open(Target::Vars(Some(VarScope::Env(n2.clone()))), window, cx)
                                        })),
                                )
                            })
                            .child(
                                Button::new(("env-more", ix))
                                    .small()
                                    .ghost()
                                    .icon(IconName::Ellipsis)
                                    .dropdown_menu(move |menu, _, _| env_menu(menu, &view, n3.clone(), is_default, current)),
                            )
                    }))
                    .into_any_element()
            }
        };
        v_flex()
            .id("env-manager")
            .size_full()
            .overflow_y_scroll()
            .p_5()
            .gap_4()
            .child(header)
            .child(body)

            .into_any_element()
    }

    pub(super) fn set_default_env(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        match envs::set_default(self.project.root(), &name) {
            Ok(()) => {
                self.reload_files(window, cx);
                window.push_notification(Notification::success(format!("默认环境：{name}（不带 -e 的命令行运行使用它）")), cx);
            }
            Err(e) => window.push_notification(Notification::error(format!("{e:#}")), cx),
        }
    }

    pub(super) fn rename_env(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let (root, this) = (self.project.root().to_path_buf(), cx.entity().downgrade());
        dialogs::prompt_text(
            window,
            cx,
            format!("重命名环境「{name}」"),
            "envs/<名称>.json 与本机覆盖一起改名；默认环境与固定在它上面的流程自动同步",
            name.clone(),
            "重命名",
            move |new_name, window, cx| {
                envs::rename(&root, &name, &new_name)?;
                let new_name = new_name.trim().to_string();
                let _ = this.update(cx, |w, cx| {
                    let was_current = w.env == name;
                    let editing = matches!(&w.center, Center::Vars(p) if matches!(p.read(cx).active(), Some(VarScope::Env(e) | VarScope::Local(e)) if *e == name));
                    w.reload_files(window, cx);
                    if was_current {
                        w.switch_env(new_name.clone(), window, cx);
                    }
                    if editing {
                        w.open_now(Target::Vars(Some(VarScope::Env(new_name.clone()))), window, cx);
                    }
                    window.push_notification(Notification::success(format!("已重命名环境 {name} → {new_name}")), cx);
                });
                Ok(())
            },
        );
    }

    pub(super) fn delete_env(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let (root, this) = (self.project.root().to_path_buf(), cx.entity().downgrade());
        let local = requestbaby_core::config::local_env_path(&root, &name).exists();
        dialogs::confirm_danger(
            window,
            cx,
            format!("删除环境「{name}」？"),
            if local {
                format!("envs/{name}.json 与本机覆盖 envs/{name}.local.json 将移到废纸篓，可在 Finder 中还原。")
            } else {
                format!("envs/{name}.json 将移到废纸篓，可在 Finder 中还原。")
            },
            "移到废纸篓",
            move |window, cx| {
                let _ = this.update(cx, |w, cx| match envs::delete(&root, &name) {
                    Ok(_) => {
                        if matches!(&w.center, Center::Vars(p) if matches!(p.read(cx).active(), Some(VarScope::Env(e) | VarScope::Local(e)) if *e == name)) {
                            w.open_now(Target::Envs, window, cx);
                        }
                        w.reload_files(window, cx);
                        window.push_notification(Notification::success(format!("已删除环境 {name}")), cx);
                    }
                    Err(e) => window.push_notification(Notification::error(format!("{e:#}")), cx),
                });
            },
        );
    }
}

/// 环境操作菜单：环境管理页「…」与侧栏右键共用
pub(super) fn env_menu(menu: PopupMenu, view: &WeakEntity<Workspace>, name: String, is_default: bool, current: bool) -> PopupMenu {
    let (n1, n2, n3, n4, n5, n6) = (name.clone(), name.clone(), name.clone(), name.clone(), name.clone(), name.clone());
    let mut menu = menu;
    if !current {
        menu = menu.item(menu_item(view, "切换到此环境", IconName::Check, move |w, window, cx| w.switch_env(n1.clone(), window, cx)));
    }
    // 只编辑当前环境的变量：别的环境先切换过去
    let edit_label = if current { "编辑变量" } else { "切换并编辑变量" };
    menu = menu.item(menu_item(view, edit_label, IconName::Settings2, move |w, window, cx| {
        w.open(Target::Vars(Some(VarScope::Env(n2.clone()))), window, cx)
    }));
    if !is_default {
        menu = menu.item(menu_item(view, "设为默认环境", IconName::Star, move |w, window, cx| w.set_default_env(n3.clone(), window, cx)));
    }
    menu.separator()
        .item(menu_item(view, "重命名…", IconName::Replace, move |w, window, cx| w.rename_env(n4.clone(), window, cx)))
        .item(menu_item(view, "复制为新环境…", IconName::Copy, move |w, window, cx| w.new_env(Some(n5.clone()), window, cx)))
        .separator()
        .item(
            PopupMenuItem::new(if is_default { "删除…（默认环境需先改默认）" } else { "删除…" })
                .icon(IconName::Delete)
                .disabled(is_default)
                .on_click({
                    let view = view.clone();
                    move |_, window, cx| {
                        let _ = view.update(cx, |w, cx| w.delete_env(n6.clone(), window, cx));
                    }
                }),
        )
}
