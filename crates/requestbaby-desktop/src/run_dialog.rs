//! 运行配置：「运行参数」与 `requestbaby run` 的参数一一对应并实时展示等价命令；
//! 「变量」直接嵌入变量面板（默认打开本次运行环境那一层），改完即可运行。
use crate::vars_panel::VarsPanel;
use gpui_kit::component::{
    ActiveTheme as _, IndexPath, Sizable as _, h_flex,
    input::{Input, InputEvent, InputState},
    select::{Select, SelectEvent, SelectState},
    switch::Switch,
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_kit::{
    AppContext as _, Context, Entity, FontWeight, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use requestbaby_core::session::RunRequest;
use requestbaby_core::varstore::VarScope;
use std::path::PathBuf;

/// 运行配置弹窗的入参
pub struct RunDialogInit {
    pub root: PathBuf,
    /// 可选运行目标（集合目录与用例，相对项目根）
    pub targets: Vec<String>,
    pub envs: Vec<String>,
    pub env: String,
    pub default_env: String,
    /// 上次的请求（或从树上带入的目标）
    pub initial: RunRequest,
}

pub struct RunOptionsPanel {
    tab: usize,
    target: Entity<SelectState<Vec<SharedString>>>,
    env_select: Entity<SelectState<Vec<SharedString>>>,
    vars: Entity<VarsPanel>,
    tags: Entity<InputState>,
    name: Entity<InputState>,
    parallel: Entity<InputState>,
    output: Entity<InputState>,
    fail_fast: bool,
    failed: bool,
    dry_run: bool,
    env: String,
    _subscriptions: Vec<Subscription>,
}

impl RunOptionsPanel {
    pub fn new(init: RunDialogInit, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let RunDialogInit { root, targets, envs, env, default_env, initial } = init;
        let initial = &initial;
        let env_items: Vec<SharedString> = envs.iter().map(|e| SharedString::from(e.clone())).collect();
        let env_ix = envs.iter().position(|e| *e == env).map(IndexPath::new);
        let env_select = cx.new(|cx| SelectState::new(env_items, env_ix, window, cx));
        let scope = Some(VarScope::Env(env.clone()));
        let (vars_env, vars_default) = (env.clone(), default_env.clone());
        let vars = cx.new(|cx| VarsPanel::new(root, vars_env, vars_default, scope, true, window, cx));
        let first = initial.paths_or_default().first().cloned().unwrap_or_else(|| "collections".into());
        let items: Vec<SharedString> = targets.into_iter().map(SharedString::from).collect();
        let ix = items.iter().position(|t| t.as_ref() == first).map(IndexPath::new).or(Some(IndexPath::new(0)));
        let target = cx.new(|cx| SelectState::new(items, ix, window, cx).searchable(true));
        let text = |v: Option<String>| v.unwrap_or_default();
        let tags_v = initial.normalized_tags().join(", ");
        let name_v = text(initial.name.clone());
        let par_v = initial.parallel.map(|p| p.to_string()).unwrap_or_default();
        let out_v = text(initial.output.clone());
        let tags = cx.new(|cx| InputState::new(window, cx).placeholder("smoke, auth（任一命中即选）").default_value(tags_v));
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("用例名或文件名子串").default_value(name_v));
        let parallel = cx.new(|cx| InputState::new(window, cx).placeholder("留空 = 串行").default_value(par_v));
        let output = cx.new(|cx| InputState::new(window, cx).placeholder("如 responses（相对项目根，每用例一个 JSON）").default_value(out_v));
        let mut subs = vec![];
        for i in [&tags, &name, &parallel, &output] {
            subs.push(cx.subscribe_in(i, window, |_, _, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    cx.notify();
                }
            }));
        }
        subs.push(cx.subscribe_in(&target, window, |_, _, _: &SelectEvent<Vec<SharedString>>, _, cx| cx.notify()));
        subs.push(cx.subscribe_in(&env_select, window, move |this, _, ev: &SelectEvent<Vec<SharedString>>, window, cx| {
            if let SelectEvent::Confirm(Some(e)) = ev {
                this.env = e.to_string();
                let (env, default_env) = (this.env.clone(), default_env.clone());
                this.vars.update(cx, |p, cx| p.set_env(env, default_env, window, cx));
                cx.notify();
            }
        }));
        Self {
            tab: 0,
            target,
            env_select,
            vars,
            tags,
            name,
            parallel,
            output,
            fail_fast: initial.fail_fast,
            failed: initial.failed,
            dry_run: initial.dry_run,
            env,
            _subscriptions: subs,
        }
    }

    /// 运行前保存变量页中未保存的修改；失败时不应开始运行
    pub fn save_vars(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.vars.update(cx, |p, cx| p.save(window, cx))
    }

    /// 当前表单 → 运行请求（parallel 非数字时忽略）
    pub fn request(&self, cx: &gpui_kit::App) -> RunRequest {
        let s = |e: &Entity<InputState>| e.read(cx).value().trim().to_string();
        let opt = |v: String| (!v.is_empty()).then_some(v);
        RunRequest {
            paths: vec![self.target.read(cx).selected_value().map(|v| v.to_string()).unwrap_or_else(|| "collections".into())],
            env: Some(self.env.clone()),
            tags: opt(s(&self.tags)).into_iter().collect(),
            name: opt(s(&self.name)),
            fail_fast: self.fail_fast,
            failed: self.failed,
            dry_run: self.dry_run,
            verbose: false,
            parallel: s(&self.parallel).parse().ok().filter(|n: &usize| *n > 0),
            output: opt(s(&self.output)),
            headed: false,
        }
    }
}

impl Render for RunOptionsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let vars_dirty = self.vars.read(cx).is_dirty();
        v_flex()
            .gap_3()
            .child(
                TabBar::new("run-dialog-tabs")
                    .underline()
                    .small()
                    .selected_index(self.tab)
                    .child(Tab::new().label("运行参数"))
                    .child(Tab::new().label(if vars_dirty { format!("变量 · 环境 {} · 未保存", self.env) } else { format!("变量 · 环境 {}", self.env) }))
                    .on_click(cx.listener(|this, ix: &usize, _, cx| {
                        this.tab = *ix;
                        cx.notify();
                    })),
            )
            .map(|this| match self.tab {
                0 => this.child(self.render_params(cx).into_any_element()),
                _ => this.child(div().h(px(460.)).child(self.vars.clone()).into_any_element()),
            })
    }
}

impl RunOptionsPanel {
    fn render_params(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let label = |t: &'static str| div().text_xs().font_weight(FontWeight::MEDIUM).text_color(muted).child(t);
        let command = self.request(cx).command_line();
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_3()
                    .child(v_flex().flex_1().gap_1().child(label("运行目标")).child(Select::new(&self.target).small()))
                    .child(v_flex().w(px(160.)).gap_1().child(label("环境 -e")).child(Select::new(&self.env_select).small())),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(v_flex().flex_1().gap_1().child(label("标签 --tag")).child(Input::new(&self.tags).small()))
                    .child(v_flex().flex_1().gap_1().child(label("名称 --name")).child(Input::new(&self.name).small())),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(v_flex().w(px(160.)).gap_1().child(label("并发 --parallel")).child(Input::new(&self.parallel).small()))
                    .child(v_flex().flex_1().gap_1().child(label("响应落盘 -o")).child(Input::new(&self.output).small())),
            )
            .child(
                h_flex()
                    .gap_6()
                    .child(
                        Switch::new("opt-fail-fast")
                            .small()
                            .label("首个失败即中止 --fail-fast")
                            .checked(self.fail_fast)
                            .on_change(cx.listener(|this, v: &bool, _, cx| {
                                this.fail_fast = *v;
                                cx.notify();
                            })),
                    )
                    .child(
                        Switch::new("opt-failed")
                            .small()
                            .label("只重跑最近失败（含前置依赖）--failed")
                            .checked(self.failed)
                            .on_change(cx.listener(|this, v: &bool, _, cx| {
                                this.failed = *v;
                                cx.notify();
                            })),
                    )
                    .child(
                        Switch::new("opt-dry-run")
                            .small()
                            .label("只编译不发送 --dry-run")
                            .checked(self.dry_run)
                            .on_change(cx.listener(|this, v: &bool, _, cx| {
                                this.dry_run = *v;
                                cx.notify();
                            })),
                    ),
            )
            .child(h_flex().child(crate::style::cli_button("run-cli", command).label("复制等价命令")))
    }
}
