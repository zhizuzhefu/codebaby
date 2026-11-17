//! 侧栏：集合树（右键菜单：发送 / 运行 / 新建 / 重命名 / 删除）、工作流、环境、项目设置。
use super::{Center, Target, Workspace};
use crate::project::{CaseStatus, status_of};
use crate::run::ResultsSource;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex,
    button::{Button, ButtonVariants as _},
    list::ListItem,
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
    tag::Tag,
    tree::{TreeItem, tree},
    v_flex,
};
use gpui_kit::{
    Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, div, prelude::FluentBuilder as _, px,
};
use std::collections::{BTreeMap, HashMap, HashSet};

impl Workspace {
    /// 树节点 id：`dir:<相对项目根>` / `case:<相对项目根>`；空集合目录同样展示
    pub(super) fn rebuild_tree(&mut self, cx: &mut Context<Self>) {
        #[derive(Default)]
        struct Dir {
            dirs: BTreeMap<String, Dir>,
            files: Vec<(String, String)>,
        }
        fn node<'a>(root: &'a mut Dir, segs: &[&str]) -> &'a mut Dir {
            segs.iter().fold(root, |n, s| n.dirs.entry(s.to_string()).or_default())
        }
        let mut root = Dir::default();
        for d in self.project.collection_dirs() {
            if let Some(inner) = d.strip_prefix("collections/") {
                node(&mut root, &inner.split('/').collect::<Vec<_>>());
            }
        }
        for c in self.project.cases() {
            let inner = c.rel.strip_prefix("collections/").unwrap_or(&c.rel);
            let mut segs: Vec<&str> = inner.split('/').collect();
            let file = segs.pop().unwrap_or_default();
            node(&mut root, &segs).files.push((c.rel.clone(), file.to_string()));
        }
        fn build(prefix: &str, d: &Dir, collapsed: &HashSet<String>) -> Vec<TreeItem> {
            let mut out = vec![];
            for (name, sub) in &d.dirs {
                let path = format!("{prefix}/{name}");
                let id = format!("dir:{path}");
                let expanded = !collapsed.contains(&id);
                out.push(TreeItem::new(id, name.clone()).expanded(expanded).children(build(&path, sub, collapsed)));
            }
            for (rel, file) in &d.files {
                out.push(TreeItem::new(format!("case:{rel}"), file.clone()));
            }
            out
        }
        let items = build("collections", &root, &self.collapsed);
        let selected = match &self.center {
            Center::Case => self.editor.read(cx).rel().map(|r| format!("case:{r}")),
            Center::Collection(dir, _) => Some(format!("dir:{dir}")),
            _ => None,
        };
        self.tree.update(cx, |t, cx| {
            t.set_items(items, cx);
            let ix = selected.and_then(|id| t.index_of(&SharedString::from(id)));
            t.set_selected_index(ix, cx);
        });
    }

    pub(super) fn select_in_tree(&self, id: &str, cx: &mut Context<Self>) {
        let id = SharedString::from(id.to_string());
        self.tree.update(cx, |t, cx| {
            let ix = t.index_of(&id);
            t.set_selected_index(ix, cx);
        });
    }

    pub(super) fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let statuses = self.statuses.clone();
        let live: HashMap<String, CaseStatus> = {
            let run = self.run.read(cx);
            let mut m = HashMap::new();
            let dry_run = run.report().and_then(|r| r.meta.as_ref()).is_some_and(|m| m.dry_run);
            if run.source() == &ResultsSource::Live && !dry_run {
                for r in run.rows() {
                    let st = status_of(r);
                    m.entry(r.file.clone())
                        .and_modify(|s| {
                            if st == CaseStatus::Failed {
                                *s = st;
                            }
                        })
                        .or_insert(st);
                }
            }
            m
        };
        let current = self.run.read(cx).current().map(String::from);
        let methods: HashMap<String, (String, String, bool)> = self
            .project
            .cases()
            .iter()
            .map(|c| (c.rel.clone(), (c.method.clone(), c.name.clone(), c.parse_error.is_some())))
            .collect();
        let view = cx.entity().downgrade();
        let menu_view = view.clone();
        let root = self.project.root().to_path_buf();
        let total = self.project.cases().len();

        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(
                section_header("集合", format!("{total} 个用例"), cx).child(
                    Button::new("new-in-root")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Plus)
                        .tooltip("新建")
                        .dropdown_menu({
                            let view = view.clone();
                            move |menu, _, _| {
                                menu.item(menu_item(&view, "新建用例…", IconName::File, |w, window, cx| {
                                    w.new_case_in("collections".into(), window, cx)
                                }))
                                .item(menu_item(&view, "新建集合…", IconName::Folder, |w, window, cx| {
                                    w.new_collection_in("collections".into(), window, cx)
                                }))
                                .separator()
                                .item(menu_item(&view, "根集合配置", IconName::Settings2, |w, window, cx| {
                                    w.open(Target::Collection("collections".into()), window, cx)
                                }))
                            }
                        }),
                ),
            )
            .child(
                div().flex_1().min_h_0().px_1().child(
                    tree(&self.tree, move |ix, entry, _selected, _window, cx| {
                        let item = entry.item();
                        let id = item.id.to_string();
                        let theme = cx.theme();
                        let mut row = h_flex().gap_2().w_full().min_w_0();
                        if entry.is_folder() {
                            row = row
                                .child(
                                    Icon::new(if entry.is_expanded() { IconName::FolderOpen } else { IconName::Folder })
                                        .small()
                                        .text_color(theme.muted_foreground),
                                )
                                .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(item.label.clone()));
                        } else {
                            let rel = id.trim_start_matches("case:").to_string();
                            let st = live.get(&rel).or_else(|| statuses.get(&rel)).copied();
                            let (method, name, broken) = methods.get(&rel).cloned().unwrap_or_default();
                            let (icon, color) = if current.as_deref() == Some(rel.as_str()) {
                                (IconName::LoaderCircle, theme.primary)
                            } else if broken {
                                (IconName::TriangleAlert, theme.warning)
                            } else {
                                match st {
                                    Some(CaseStatus::Passed) => (IconName::CircleCheck, theme.success),
                                    Some(CaseStatus::Failed) => (IconName::CircleX, theme.danger),
                                    Some(CaseStatus::Skipped) => (IconName::Ban, theme.muted_foreground),
                                    None => (IconName::Dash, theme.muted_foreground),
                                }
                            };
                            row = row
                                .child(Icon::new(icon).xsmall().text_color(color))
                                .child(
                                    div()
                                        .w(px(44.))
                                        .flex_shrink_0()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.muted_foreground)
                                        .child(method),
                                )
                                .child(div().flex_1().min_w_0().text_sm().overflow_hidden().text_ellipsis().whitespace_nowrap().child(name))
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(item.label.trim_end_matches(".json").split('-').next().unwrap_or_default().to_string()),
                                );
                        }
                        let view = view.clone();
                        ListItem::new(ix)
                            .w_full()
                            .rounded(cx.theme().radius)
                            .px_2()
                            .pl(px(14.) * entry.depth() + px(8.))
                            .child(row)
                            .on_click(move |_, window, cx| {
                                if let Some(rel) = id.strip_prefix("case:") {
                                    let rel = rel.to_string();
                                    let _ = view.update(cx, |w, cx| w.open(Target::Case(rel), window, cx));
                                }
                            })
                    })
                    .context_menu(move |_, entry, menu, _, _| {
                        let id = entry.item().id.to_string();
                        let view = &menu_view;
                        if let Some(rel) = id.strip_prefix("case:") {
                            let path = root.join(rel);
                            case_menu(menu, view, rel.to_string(), path)
                        } else if let Some(dir) = id.strip_prefix("dir:") {
                            dir_menu(menu, view, dir.to_string(), root.join(dir))
                        } else {
                            menu
                        }
                    }),
                ),
            )
            .child(self.render_workflows(cx))
            .child(self.render_envs(cx))
            .child(self.render_project_entry(cx))
    }

    fn render_workflows(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let workflows = self.project.config().workflows.clone();
        let running = self.run.read(cx).is_running();
        let active = self.run.read(cx).workflow().map(String::from);
        let muted = cx.theme().muted_foreground;
        let hover = cx.theme().sidebar_accent;
        v_flex()
            .px_1()
            .pb_1()
            .border_t_1()
            .border_color(cx.theme().sidebar_border)
            .child(
                section_header("工作流", format!("{} 个", workflows.len()), cx).child(
                    Button::new("edit-workflows")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Settings2)
                        .tooltip("在项目设置中编排（requestbaby.json · workflows）")
                        .on_click(cx.listener(|this, _, window, cx| this.open(Target::Project, window, cx))),
                ),
            )
            .when(workflows.is_empty(), |this| {
                this.child(div().px_3().pb_1().text_xs().text_color(muted).child("尚未编排：在项目设置中按步骤组合集合 / 用例"))
            })
            .children(workflows.into_iter().enumerate().map(|(ix, (key, wf))| {
                let label = if wf.name.is_empty() { key.clone() } else { wf.name.clone() };
                let is_active = active.as_deref() == Some(key.as_str());
                let (run_key, dry_key) = (key.clone(), key.clone());
                h_flex()
                    .id(("workflow", ix))
                    .px_2()
                    .py_1()
                    .gap_2()
                    .rounded(cx.theme().radius)
                    .hover(|this| this.bg(hover))
                    .child(Icon::new(IconName::Network).xsmall().text_color(if is_active { cx.theme().primary } else { muted }))
                    .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().text_sm().child(label))
                    .child(div().text_xs().text_color(muted).child(format!("{} 步", wf.steps.len())))
                    .child(
                        Button::new(("wf-dry", ix))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Eye)
                            .tooltip(format!("requestbaby workflow {key} --dry-run（只编译不发送）"))
                            .disabled(running)
                            .on_click(cx.listener(move |this, _, window, cx| this.run_workflow(dry_key.clone(), true, window, cx))),
                    )
                    .child(
                        Button::new(("wf-run", ix))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Play)
                            .tooltip(format!("requestbaby workflow {key}"))
                            .disabled(running)
                            .on_click(cx.listener(move |this, _, window, cx| this.run_workflow(run_key.clone(), false, window, cx))),
                    )
            }))
    }

    fn render_envs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let envs = self.project.envs().to_vec();
        let default = self.project.config().default_env.clone();
        let open = match &self.center {
            Center::Env(name, _) => Some(name.clone()),
            _ => None,
        };
        let muted = cx.theme().muted_foreground;
        let (hover, active) = (cx.theme().sidebar_accent, cx.theme().sidebar_accent);
        v_flex()
            .px_1()
            .pb_1()
            .border_t_1()
            .border_color(cx.theme().sidebar_border)
            .child(
                section_header("环境", format!("当前 {}", self.env), cx).child(
                    Button::new("new-env")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Plus)
                        .tooltip("新建环境（requestbaby new env）")
                        .on_click(cx.listener(|this, _, window, cx| this.new_env(None, window, cx))),
                ),
            )
            .children(envs.into_iter().enumerate().map(|(ix, name)| {
                let is_current = name == self.env;
                let target = name.clone();
                h_flex()
                    .id(("env", ix))
                    .px_2()
                    .py_1()
                    .gap_2()
                    .rounded(cx.theme().radius)
                    .when(open.as_deref() == Some(name.as_str()), |this| this.bg(active))
                    .hover(|this| this.bg(hover))
                    .on_click(cx.listener(move |this, _, window, cx| this.open(Target::Env(target.clone()), window, cx)))
                    .child(
                        Icon::new(if is_current { IconName::Check } else { IconName::Globe })
                            .xsmall()
                            .text_color(if is_current { cx.theme().primary } else { muted }),
                    )
                    .child(div().flex_1().text_sm().child(name.clone()))
                    .when(name == default, |this| this.child(Tag::secondary().xsmall().child("默认")))
            }))
    }

    fn render_project_entry(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let open = matches!(self.center, Center::Project(_));
        let file = self.project.rel_of(&self.project.project_file());
        div().p_1().border_t_1().border_color(cx.theme().sidebar_border).child(
            h_flex()
                .id("project-settings")
                .px_2()
                .py_1()
                .gap_2()
                .rounded(cx.theme().radius)
                .when(open, |this| this.bg(cx.theme().sidebar_accent))
                .hover(|this| this.bg(cx.theme().sidebar_accent))
                .on_click(cx.listener(|this, _, window, cx| this.open(Target::Project, window, cx)))
                .child(Icon::new(IconName::Settings).xsmall().text_color(cx.theme().muted_foreground))
                .child(div().flex_1().text_sm().child("项目设置"))
                .child(div().text_xs().text_color(cx.theme().muted_foreground).child(file)),
        )
    }
}

fn section_header(title: &'static str, meta: String, cx: &Context<Workspace>) -> gpui_kit::Div {
    h_flex()
        .px_3()
        .pt_2()
        .pb_1()
        .gap_2()
        .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(cx.theme().sidebar_foreground).child(title))
        .child(div().flex_1().text_xs().text_color(cx.theme().muted_foreground).child(meta))
}

pub(super) fn menu_item(
    view: &WeakEntity<Workspace>,
    label: impl Into<SharedString>,
    icon: IconName,
    f: impl Fn(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static,
) -> PopupMenuItem {
    let view = view.clone();
    PopupMenuItem::new(label).icon(icon).on_click(move |_, window, cx| {
        let _ = view.update(cx, |w, cx| f(w, window, cx));
    })
}

fn case_menu(menu: PopupMenu, view: &WeakEntity<Workspace>, rel: String, path: std::path::PathBuf) -> PopupMenu {
    let (r1, r2, p1, p2, p3) = (rel.clone(), rel.clone(), path.clone(), path.clone(), path);
    let name = rel.rsplit('/').next().unwrap_or_default().to_string();
    menu.item(menu_item(view, "发送（不写历史）", IconName::Play, move |w, window, cx| w.send_rel(r1.clone(), window, cx)))
        .item(menu_item(view, "运行并记录", IconName::CircleCheck, move |w, window, cx| w.run_paths(vec![r2.clone()], window, cx)))
        .item(menu_item(view, "运行配置…", IconName::Settings2, move |w, window, cx| {
            let req = requestbaby_core::session::RunRequest { paths: vec![rel.clone()], ..Default::default() };
            w.open_run_dialog(Some(req), window, cx)
        }))
        .separator()
        .item(menu_item(view, "重命名…", IconName::Replace, move |w, window, cx| w.rename_path(p1.clone(), window, cx)))
        .item(PopupMenuItem::new("在 Finder 中显示").icon(IconName::Folder).on_click(move |_, _, cx| cx.reveal_path(&p2)))
        .separator()
        .item(menu_item(view, "删除", IconName::Delete, move |w, window, cx| {
            w.delete_path(p3.clone(), format!("用例「{name}」"), window, cx)
        }))
}

fn dir_menu(menu: PopupMenu, view: &WeakEntity<Workspace>, dir: String, path: std::path::PathBuf) -> PopupMenu {
    let (d1, d2, d3, d4, d5) = (dir.clone(), dir.clone(), dir.clone(), dir.clone(), dir.clone());
    let (p1, p2, p3) = (path.clone(), path.clone(), path);
    let name = dir.rsplit('/').next().unwrap_or_default().to_string();
    menu.item(menu_item(view, "运行此集合", IconName::Play, move |w, window, cx| w.run_paths(vec![d1.clone()], window, cx)))
        .item(menu_item(view, "运行配置…", IconName::Settings2, move |w, window, cx| {
            let req = requestbaby_core::session::RunRequest { paths: vec![d2.clone()], ..Default::default() };
            w.open_run_dialog(Some(req), window, cx)
        }))
        .item(menu_item(view, "集合配置", IconName::Settings, move |w, window, cx| w.open(Target::Collection(d3.clone()), window, cx)))
        .separator()
        .item(menu_item(view, "新建用例…", IconName::File, move |w, window, cx| w.new_case_in(d4.clone(), window, cx)))
        .item(menu_item(view, "新建子集合…", IconName::Folder, move |w, window, cx| w.new_collection_in(d5.clone(), window, cx)))
        .separator()
        .item(menu_item(view, "重命名…", IconName::Replace, move |w, window, cx| w.rename_path(p1.clone(), window, cx)))
        .item(PopupMenuItem::new("在 Finder 中显示").icon(IconName::Folder).on_click(move |_, _, cx| cx.reveal_path(&p2)))
        .separator()
        .item(menu_item(view, "删除", IconName::Delete, move |w, window, cx| {
            w.delete_path(p3.clone(), format!("集合「{name}」"), window, cx)
        }))
}
