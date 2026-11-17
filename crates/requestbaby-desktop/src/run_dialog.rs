//! 运行配置：与 `requestbaby run` 的参数一一对应，并实时展示等价命令。
use gpui_kit::component::{
    ActiveTheme as _, IndexPath, Sizable as _, h_flex,
    clipboard::Clipboard,
    input::{Input, InputEvent, InputState},
    select::{Select, SelectEvent, SelectState},
    switch::Switch,
    v_flex,
};
use gpui_kit::{
    AppContext as _, Context, Entity, FontWeight, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Window, div, px,
};
use requestbaby_core::session::RunRequest;

pub struct RunOptionsPanel {
    target: Entity<SelectState<Vec<SharedString>>>,
    tags: Entity<InputState>,
    name: Entity<InputState>,
    parallel: Entity<InputState>,
    output: Entity<InputState>,
    fail_fast: bool,
    dry_run: bool,
    env: String,
    _subscriptions: Vec<Subscription>,
}

impl RunOptionsPanel {
    /// `targets`：可选运行目标（集合目录与用例，相对项目根）；`initial`：上次的请求
    pub fn new(targets: Vec<String>, initial: &RunRequest, env: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
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
        Self {
            target,
            tags,
            name,
            parallel,
            output,
            fail_fast: initial.fail_fast,
            dry_run: initial.dry_run,
            env,
            _subscriptions: subs,
        }
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
            dry_run: self.dry_run,
            verbose: false,
            parallel: s(&self.parallel).parse().ok().filter(|n: &usize| *n > 0),
            output: opt(s(&self.output)),
        }
    }
}

impl Render for RunOptionsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let label = |t: &'static str| div().text_xs().font_weight(FontWeight::MEDIUM).text_color(muted).child(t);
        let command = self.request(cx).command_line();
        v_flex()
            .gap_3()
            .child(v_flex().gap_1().child(label("运行目标")).child(Select::new(&self.target).small()))
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
            .child(
                v_flex()
                    .gap_1()
                    .child(label("等价命令"))
                    .child(
                        h_flex()
                            .gap_2()
                            .p_2()
                            .rounded(cx.theme().radius)
                            .bg(cx.theme().muted)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_xs()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .child(command.clone()),
                            )
                            .child(Clipboard::new("copy-run-command").value(SharedString::from(command))),
                    ),
            )
    }
}
