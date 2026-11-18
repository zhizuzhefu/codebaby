//! 侧栏：集合树（右键菜单：发送 / 运行 / 新建 / 重命名 / 删除）、流程、环境、项目设置。
use crate::style;
use super::{Center, Target, Workspace};
use crate::new_case_dialog::NewCaseMode;
use requestbaby_core::varstore::VarScope;
use crate::project::{CaseStatus, status_of};
use crate::run::ResultsSource;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex,
    button::{Button, ButtonVariants as _},
    input::Input,
    list::ListItem,
    menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem},
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
        let query = self.tree_filter.read(cx).value().trim().to_lowercase();
        let filtering = !query.is_empty();
        // 空格分隔的多个词须全部命中（名称 / 路径 / 方法 / URL，不区分大小写）
        let matches = |c: &crate::project::CaseInfo| case_matches(c, &query);
        let mut root = Dir::default();
        if !filtering {
            for d in self.project.collection_dirs() {
                if let Some(inner) = d.strip_prefix("collections/") {
                    node(&mut root, &inner.split('/').collect::<Vec<_>>());
                }
            }
        }
        for c in self.project.cases().iter().filter(|c| !filtering || matches(c)) {
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
        // 筛选时全部展开，命中项一目了然
        let no_collapse = HashSet::new();
        let items = build("collections", &root, if filtering { &no_collapse } else { &self.collapsed });
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
        let workflow_keys: Vec<String> = self.project.config().workflows.keys().cloned().collect();
        let total = self.project.cases().len();
        let query = self.tree_filter.read(cx).value().trim().to_lowercase();
        let filtering = !query.is_empty();
        let no_match = filtering && !self.project.cases().iter().any(|c| case_matches(c, &query));

        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(
                section_header("集合", format!("{total} 个用例"), cx).child(
                    new_button("new-in-root")
                        .tooltip("新建用例 / 从 cURL 新建 / 新建集合")
                        .dropdown_menu({
                            let view = view.clone();
                            move |menu, _, _| {
                                menu.item(menu_item(&view, "新建用例…", IconName::File, |w, window, cx| {
                                    w.new_case_in("collections".into(), NewCaseMode::Http, window, cx)
                                }))
                                .item(menu_item(&view, "从 cURL 新建用例…", IconName::SquareTerminal, |w, window, cx| {
                                    w.new_case_in("collections".into(), NewCaseMode::Curl, window, cx)
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
            .child(div().px_2().pb_1().child(Input::new(&self.tree_filter).xsmall().prefix(Icon::new(IconName::Search).xsmall()).cleanable(true)))
            .when(no_match, |this| {
                this.child(div().px_3().py_2().text_xs().text_color(cx.theme().muted_foreground).child("没有匹配的用例"))
            })
            .child(
                div().flex_1().min_h_0().px_1().child(
                    tree(&self.tree, move |ix, entry, _selected, _window, cx| {
                        let item = entry.item();
                        let id = item.id.to_string();
                        let theme = cx.theme();
                        let mut row = h_flex().gap_2().w_full().min_w_0();
                        if entry.is_folder() || id.starts_with("dir:") {
                            row = row
                                .child(
                                    Icon::new(if entry.is_expanded() { IconName::FolderOpen } else { IconName::Folder })
                                        .small()
                                        .text_color(theme.muted_foreground),
                                )
                                .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().text_sm().font_weight(FontWeight::MEDIUM).child(item.label.clone()));
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
                                .child(style::method_label(&method, cx).w(px(40.)).flex_shrink_0())
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
                        let (target, menu) = if let Some(rel) = id.strip_prefix("case:") {
                            let path = root.join(rel);
                            (rel.to_string(), case_menu(menu, view, rel.to_string(), path))
                        } else if let Some(dir) = id.strip_prefix("dir:") {
                            (dir.to_string(), dir_menu(menu, view, dir.to_string(), root.join(dir)))
                        } else {
                            return menu;
                        };
                        add_to_workflow_menu(menu.separator(), view, target, &workflow_keys)
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
        let active_run = self.run.read(cx).workflow().map(String::from);
        let editing: Option<String> = match &self.center {
            Center::Workflow(e) => e.read(cx).key().map(String::from),
            _ => None,
        };
        let muted = cx.theme().muted_foreground;
        let hover = cx.theme().sidebar_accent;
        let view = cx.entity().downgrade();
        v_flex()
            .px_1()
            .pb_1()
            .border_t_1()
            .border_color(cx.theme().sidebar_border)
            .child(
                section_header("流程", format!("{} 个", workflows.len()), cx).child(
                    new_button("new-workflow")
                        .tooltip("新建流程（requestbaby workflow new）")
                        .on_click(cx.listener(|this, _, window, cx| this.open(Target::Workflow(None), window, cx))),
                ),
            )
            .when(workflows.is_empty(), |this| {
                this.child(div().px_3().pb_1().text_xs().text_color(muted).child("把集合 / 用例按步骤串起来：点「新建」，或在用例树上右键「添加到流程」"))
            })
            .children(workflows.into_iter().enumerate().map(|(ix, (key, wf))| {
                let label = if wf.name.is_empty() { key.clone() } else { wf.name.clone() };
                let is_running = active_run.as_deref() == Some(key.as_str()) && running;
                let is_open = editing.as_deref() == Some(key.as_str());
                let (run_key, dry_key, open_key, menu_key) = (key.clone(), key.clone(), key.clone(), key.clone());
                let view = view.clone();
                h_flex()
                    .id(("workflow", ix))
                    .px_2()
                    .py_1()
                    .gap_2()
                    .rounded(cx.theme().radius)
                    .cursor_pointer()
                    .when(is_open, |this| this.bg(hover))
                    .hover(|this| this.bg(hover))
                    .child(Icon::new(if is_running { IconName::LoaderCircle } else { IconName::Network }).xsmall().text_color(if is_open || is_running {
                        cx.theme().primary
                    } else {
                        muted
                    }))
                    .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().text_sm().child(label))
                    .when_some(wf.env.clone(), |this, e| this.child(Tag::info().outline().xsmall().child(e)))
                    .child(div().text_xs().text_color(muted).child(format!("{} 步", wf.steps.len())))
                    .child(
                        Button::new(("wf-dry", ix))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Eye)
                            .tooltip(format!("requestbaby workflow {key} --dry-run（只编译不发送）"))
                            .disabled(running)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.run_workflow(dry_key.clone(), true, window, cx)
                            })),
                    )
                    .child(
                        Button::new(("wf-run", ix))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Play)
                            .tooltip(format!("requestbaby workflow {key}"))
                            .disabled(running)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.run_workflow(run_key.clone(), false, window, cx)
                            })),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| this.open(Target::Workflow(Some(open_key.clone())), window, cx)))
                    .context_menu(move |menu, _, _| workflow_menu(menu, &view, menu_key.clone()))
            }))
    }

    /// 变量与环境：总览 / 项目变量 / 当前环境（及本机覆盖），均打开变量面板对应的层
    fn render_envs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let default = self.project.config().default_env.clone();
        let open: Option<Option<VarScope>> = match &self.center {
            Center::Vars(p) => Some(p.read(cx).active().cloned()),
            _ => None,
        };
        let muted = cx.theme().muted_foreground;
        let active_bg = cx.theme().sidebar_accent;
        let view = cx.entity().downgrade();
        let mut entries: Vec<(Option<VarScope>, IconName, String)> = vec![
            (None, IconName::LayoutDashboard, "变量总览".into()),
            (Some(VarScope::Project), IconName::Settings2, "项目变量".into()),
        ];
        // 只列当前环境：切换在标题栏，其余环境的增删改在环境管理页
        let e = self.env.clone();
        if self.project.envs().contains(&e) {
            entries.push((Some(VarScope::Env(e.clone())), IconName::Globe, format!("环境变量 · {e}")));
            if requestbaby_core::config::local_env_path(self.project.root(), &e).exists() {
                entries.push((Some(VarScope::Local(e.clone())), IconName::HardDrive, format!("本机覆盖 · {e}")));
            }
        }
        v_flex()
            .px_1()
            .pb_1()
            .border_t_1()
            .border_color(cx.theme().sidebar_border)
            .child(
                section_header("变量与环境", format!("当前 {}", self.env), cx)
                    .child(
                        Button::new("manage-envs-side")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Globe)
                            .tooltip("环境管理（requestbaby env list）")
                            .on_click(cx.listener(|this, _, window, cx| this.open(Target::Envs, window, cx))),
                    )
                    .child(
                        new_button("new-env")
                            .tooltip("新建环境（requestbaby env new）")
                            .on_click(cx.listener(|this, _, window, cx| this.new_env(None, window, cx))),
                    ),
            )
            .children(entries.into_iter().enumerate().map(|(ix, (scope, icon, label))| {
                let is_open = open.as_ref() == Some(&scope);
                let env_name = match &scope {
                    Some(VarScope::Env(e)) => Some(e.clone()),
                    _ => None,
                };
                let is_current = env_name.as_deref() == Some(self.env.as_str());
                let is_default = env_name.as_deref() == Some(default.as_str());
                let menu_env = env_name.clone();
                let view = view.clone();
                let row = h_flex()
                    .id(("vars-entry", ix))
                    .px_2()
                    .py_1()
                    .gap_2()
                    .rounded(cx.theme().radius)
                    .when(is_open, |this| this.bg(active_bg))
                    .hover(|this| this.bg(active_bg))
                    .on_click(cx.listener(move |this, _, window, cx| this.open(Target::Vars(scope.clone()), window, cx)))
                    .child(Icon::new(icon).xsmall().text_color(if is_current { cx.theme().primary } else { muted }))
                    .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().text_sm().child(label))
                    .when(is_default, |this| this.child(Tag::secondary().xsmall().flex_shrink_0().child("默认")));
                // 环境行右键：与环境管理页「…」同一菜单
                match menu_env {
                    Some(name) => row
                        .context_menu(move |menu, _, _| super::envs::env_menu(menu, &view, name.clone(), is_default, is_current))
                        .into_any_element(),
                    None => row.into_any_element(),
                }
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

/// 各分区的「新建」入口：与「发送」同一样式（主色胶囊 + 图标 + 文字），一眼能找到
fn new_button(id: &'static str) -> Button {
    Button::new(id).small().primary().rounded(style::PILL).icon(IconName::Plus).label("新建")
}

fn section_header(title: &'static str, meta: String, cx: &Context<Workspace>) -> gpui_kit::Div {
    h_flex()
        .items_center()
        .px_3()
        .pt_2()
        .pb_1()
        .gap_2()
        .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(cx.theme().sidebar_foreground).child(title))
        .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().text_xs().text_color(cx.theme().muted_foreground).child(meta))
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

fn workflow_menu(menu: PopupMenu, view: &WeakEntity<Workspace>, key: String) -> PopupMenu {
    let (k1, k2, k3, k4, k5, k6) = (key.clone(), key.clone(), key.clone(), key.clone(), key.clone(), key);
    menu.item(menu_item(view, "运行", IconName::Play, move |w, window, cx| w.run_workflow(k1.clone(), false, window, cx)))
        .item(menu_item(view, "Dry-run（只编译不发送）", IconName::Eye, move |w, window, cx| w.run_workflow(k2.clone(), true, window, cx)))
        .separator()
        .item(menu_item(view, "编辑", IconName::Settings2, move |w, window, cx| w.open(Target::Workflow(Some(k3.clone())), window, cx)))
        .item(menu_item(view, "重命名…", IconName::Replace, move |w, window, cx| w.rename_workflow(k6.clone(), window, cx)))
        .item(menu_item(view, "复制为新流程…", IconName::Copy, move |w, window, cx| w.duplicate_workflow(k4.clone(), window, cx)))
        .separator()
        .item(menu_item(view, "删除…", IconName::Delete, move |w, window, cx| w.delete_workflow(k5.clone(), window, cx)))
}

/// 「添加到流程」：已有流程（追加为最后一步）+ 以它为第一步新建
fn add_to_workflow_menu(menu: PopupMenu, view: &WeakEntity<Workspace>, target: String, workflows: &[String]) -> PopupMenu {
    let mut menu = menu;
    for key in workflows {
        let (key, target) = (key.clone(), target.clone());
        menu = menu.item(menu_item(view, format!("添加到流程「{key}」"), IconName::Network, move |w, window, cx| {
            w.add_to_workflow(key.clone(), target.clone(), window, cx)
        }));
    }
    menu.item(menu_item(view, "以此新建流程…", IconName::Plus, move |w, window, cx| w.new_workflow_with(target.clone(), window, cx)))
}

fn case_menu(menu: PopupMenu, view: &WeakEntity<Workspace>, rel: String, path: std::path::PathBuf) -> PopupMenu {
    let (r1, r2, p1, p2, p3, p4) = (rel.clone(), rel.clone(), path.clone(), path.clone(), path.clone(), path);
    let name = rel.rsplit('/').next().unwrap_or_default().to_string();
    menu.item(menu_item(view, "发送（不写历史）", IconName::Play, move |w, window, cx| w.send_rel(r1.clone(), window, cx)))
        .item(menu_item(view, "运行并记录", IconName::CircleCheck, move |w, window, cx| w.run_paths(vec![r2.clone()], window, cx)))
        .item(menu_item(view, "运行配置…", IconName::Settings2, move |w, window, cx| {
            let req = requestbaby_core::session::RunRequest { paths: vec![rel.clone()], ..Default::default() };
            w.open_run_dialog(Some(req), window, cx)
        }))
        .separator()
        .item(menu_item(view, "重命名…", IconName::Replace, move |w, window, cx| w.rename_path(p1.clone(), window, cx)))
        .item(menu_item(view, "复制为新用例…", IconName::Copy, move |w, window, cx| w.duplicate_path(p4.clone(), window, cx)))
        .item(PopupMenuItem::new("在 Finder 中显示").icon(IconName::Folder).on_click(move |_, _, cx| cx.reveal_path(&p2)))
        .separator()
        .item(menu_item(view, "删除…", IconName::Delete, move |w, window, cx| {
            w.delete_path(p3.clone(), format!("用例「{name}」"), window, cx)
        }))
}

fn dir_menu(menu: PopupMenu, view: &WeakEntity<Workspace>, dir: String, path: std::path::PathBuf) -> PopupMenu {
    let (d1, d2, d3, d4, d5, d6, d7) = (dir.clone(), dir.clone(), dir.clone(), dir.clone(), dir.clone(), dir.clone(), dir.clone());
    let (p1, p2, p3, p4) = (path.clone(), path.clone(), path.clone(), path);
    let name = dir.rsplit('/').next().unwrap_or_default().to_string();
    menu.item(menu_item(view, "运行此集合", IconName::Play, move |w, window, cx| w.run_paths(vec![d1.clone()], window, cx)))
        .item(menu_item(view, "运行配置…", IconName::Settings2, move |w, window, cx| {
            let req = requestbaby_core::session::RunRequest { paths: vec![d2.clone()], ..Default::default() };
            w.open_run_dialog(Some(req), window, cx)
        }))
        .item(menu_item(view, "集合配置", IconName::Settings, move |w, window, cx| w.open(Target::Collection(d3.clone()), window, cx)))
        .item(menu_item(view, "集合变量", IconName::Settings2, move |w, window, cx| {
            w.open(Target::Vars(Some(VarScope::Collection(d6.clone()))), window, cx)
        }))
        .separator()
        .item(menu_item(view, "新建用例…", IconName::File, move |w, window, cx| w.new_case_in(d4.clone(), NewCaseMode::Http, window, cx)))
        .item(menu_item(view, "从 cURL 新建用例…", IconName::SquareTerminal, move |w, window, cx| w.new_case_in(d7.clone(), NewCaseMode::Curl, window, cx)))
        .item(menu_item(view, "新建子集合…", IconName::Folder, move |w, window, cx| w.new_collection_in(d5.clone(), window, cx)))
        .separator()
        .item(menu_item(view, "重命名…", IconName::Replace, move |w, window, cx| w.rename_path(p1.clone(), window, cx)))
        .item(menu_item(view, "复制为新集合…", IconName::Copy, move |w, window, cx| w.duplicate_path(p4.clone(), window, cx)))
        .item(PopupMenuItem::new("在 Finder 中显示").icon(IconName::Folder).on_click(move |_, _, cx| cx.reveal_path(&p2)))
        .separator()
        .item(menu_item(view, "删除…", IconName::Delete, move |w, window, cx| {
            w.delete_path(p3.clone(), format!("集合「{name}」"), window, cx)
        }))
}

/// 集合树筛选：空格分隔的多个词须全部命中（名称 / 路径 / 方法 / URL，不区分大小写；query 已小写）
fn case_matches(c: &crate::project::CaseInfo, query: &str) -> bool {
    let hay = format!("{} {} {} {}", c.name, c.rel, c.method, c.url).to_lowercase();
    query.split_whitespace().all(|w| hay.contains(w))
}
