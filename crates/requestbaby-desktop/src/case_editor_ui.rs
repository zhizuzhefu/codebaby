//! 用例编辑器里 Web / Android 的界面（与其他协议同一个编辑器、同一套断言 / 高级页，只有请求部分不同）：
//! - 地址栏：Web 写要打开的页面，Android 写应用包名；右侧「读取元素」打开页面 / 读设备当前界面，列出元素与推荐定位
//! - 步骤：一行一步（动作 + 目标 + 值），目标可从读取到的元素里点选；每行下方用人话复述这一步
//! - Android 另有「应用」页：Activity / APK / 启动前清除数据
use super::CaseEditor;
use crate::case_form::{UiStepForm, ui_has_text, ui_targets_locator};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Selectable as _, Sizable as _, h_flex,
    button::{Button, ButtonVariants as _},
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    switch::Switch,
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};
use requestbaby_core::config::{Protocol, UiAction};
use requestbaby_core::protocols::SnapElement;
use serde_json::Value;

/// 步骤里的一行
pub(super) struct UiRow {
    pub action: UiAction,
    pub target: Entity<InputState>,
    pub value: Entity<InputState>,
    pub capture: Entity<InputState>,
    pub timeout: Entity<InputState>,
    pub optional: bool,
    pub clear_data: bool,
    pub raw: Option<Value>,
}

fn target_placeholder(a: UiAction, android: bool) -> &'static str {
    match a {
        UiAction::Click | UiAction::LongPress | UiAction::Hover if android => "要点的元素：{\"id\": \"login\"} / {\"text\": \"登录\"}；直接写文字 = 按文字找；或点右侧「元素」选",
        UiAction::Click | UiAction::LongPress | UiAction::Hover => "要点的元素：{\"role\": \"button\", \"name\": \"登录\"}；直接写文字 = 按文字找；或点右侧「元素」选",
        UiAction::Fill if android => "输入框：{\"id\": \"phone\"} / {\"label\": \"手机号\"}",
        UiAction::Fill => "输入框：{\"label\": \"手机号\"} / {\"placeholder\": \"请输入手机号\"}",
        UiAction::Select => "下拉框：{\"label\": \"城市\"}",
        UiAction::WaitFor | UiAction::Expect if android => "条件：{\"text\": \"我的订单\"} / {\"activity\": \"MainActivity\"} / {\"id\": \"submit\", \"state\": \"disabled\"}",
        UiAction::WaitFor | UiAction::Expect => "条件：{\"url\": \"/orders\"} / {\"role\": \"heading\", \"name\": \"我的订单\"} / {\"text\": \"加载中\", \"state\": \"hidden\"}",
        UiAction::Goto => "地址：/settings 或完整地址；支持 {{变量}}",
        UiAction::Type => "要输入的文字（输入到当前焦点）",
        UiAction::Press if android => "按键：Back / Home / Enter / Backspace / KEYCODE_*",
        UiAction::Press => "按键：Enter / Tab / Escape / ArrowDown / Control+a",
        UiAction::Swipe => "手指移动的方向：up / down / left / right",
        UiAction::Screenshot => "这张截图的名字，如 下单成功",
        UiAction::Wait => "静候毫秒数（尽量用「等待」）",
        UiAction::Back | UiAction::Launch | UiAction::Stop => "",
    }
}

impl CaseEditor {
    fn ui_row(&mut self, f: &UiStepForm, window: &mut Window, cx: &mut Context<Self>) -> UiRow {
        let android = self.protocol == Protocol::Android;
        let target = cx.new(|cx| InputState::new(window, cx).placeholder(target_placeholder(f.action, android)).default_value(f.target.clone()));
        let value_hint = if f.action == UiAction::Select { "选项文字或值" } else { "要填的内容；支持 {{变量}}" };
        let value = cx.new(|cx| InputState::new(window, cx).placeholder(value_hint).default_value(f.value.clone()));
        let capture = cx.new(|cx| InputState::new(window, cx).placeholder("捕获（可选）").default_value(f.capture.clone()));
        let timeout = cx.new(|cx| InputState::new(window, cx).placeholder("超时 ms").default_value(f.timeout.clone()));
        for input in [&target, &value, &capture, &timeout] {
            self._ui_subs.push(cx.subscribe_in(input, window, |this, _, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.refresh_dirty(cx);
                    cx.notify();
                }
            }));
        }
        UiRow { action: f.action, target, value, capture, timeout, optional: f.optional, clear_data: f.clear_data, raw: f.raw.clone() }
    }

    pub(super) fn set_ui_steps(&mut self, steps: &[UiStepForm], window: &mut Window, cx: &mut Context<Self>) {
        self._ui_subs.clear();
        self.ui_steps = steps.iter().map(|f| self.ui_row(f, window, cx)).collect();
    }

    pub(super) fn current_ui_steps(&self, cx: &gpui_kit::App) -> Vec<UiStepForm> {
        let input = |e: &Entity<InputState>| e.read(cx).value().to_string();
        self.ui_steps
            .iter()
            .map(|r| UiStepForm {
                action: r.action,
                target: input(&r.target),
                value: input(&r.value),
                capture: input(&r.capture),
                timeout: input(&r.timeout),
                optional: r.optional,
                clear_data: r.clear_data,
                raw: r.raw.clone(),
            })
            .collect()
    }

    fn edit_ui_steps(&mut self, f: impl FnOnce(&mut Vec<UiStepForm>), window: &mut Window, cx: &mut Context<Self>) {
        let mut steps = self.current_ui_steps(cx);
        f(&mut steps);
        self.set_ui_steps(&steps, window, cx);
        self.refresh_dirty(cx);
        cx.notify();
    }

    fn set_ui_action(&mut self, ix: usize, a: UiAction, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_ui_steps(
            |s| {
                if let Some(st) = s.get_mut(ix) {
                    if st.action != a {
                        // 都以元素为目标（点击 ↔ 断言 …）时保留目标
                        let keep = ui_targets_locator(st.action) && ui_targets_locator(a);
                        let mut n = UiStepForm::new(a);
                        if keep {
                            n.target = st.target.clone();
                        }
                        n.capture = st.capture.clone();
                        n.timeout = st.timeout.clone();
                        *st = n;
                    }
                }
            },
            window,
            cx,
        );
    }

    fn move_ui_step(&mut self, ix: usize, up: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_ui_steps(
            |s| {
                let to = if up { ix.checked_sub(1) } else { Some(ix + 1).filter(|t| *t < s.len()) };
                if let Some(to) = to {
                    s.swap(ix, to);
                }
            },
            window,
            cx,
        );
    }

    fn remove_ui_step(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_ui_steps(
            |s| {
                if ix < s.len() {
                    s.remove(ix);
                }
            },
            window,
            cx,
        );
    }

    fn toggle_ui_flag(&mut self, ix: usize, clear_data: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_ui_steps(
            |s| {
                if let Some(st) = s.get_mut(ix) {
                    if clear_data {
                        st.clear_data = !st.clear_data;
                    } else {
                        st.optional = !st.optional;
                    }
                }
            },
            window,
            cx,
        );
    }

    /// 选中读取到的元素：填进这一步的目标
    fn pick_element(&mut self, ix: usize, locator: Value, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(r) = self.ui_steps.get(ix) {
            let text = serde_json::to_string(&locator).unwrap_or_default();
            r.target.update(cx, |st, cx| st.set_value(text, window, cx));
            self.refresh_dirty(cx);
            cx.notify();
        }
    }

    /// 后台读取元素：Web 打开地址栏的页面（变量按当前环境替换），Android 读设备当前界面；与 `requestbaby web / android snapshot` 同一实现
    pub(super) fn load_ui_snapshot(&mut self, cx: &mut Context<Self>) {
        let root = self.path.as_deref().and_then(requestbaby_core::runner::discover::find_project_root);
        let env = if self.env.is_empty() { "dev".to_string() } else { self.env.clone() };
        let url = self.ui_target.read(cx).value().to_string();
        let android = self.protocol == Protocol::Android;
        self.ui_snapshot_loading = true;
        let task = cx.background_spawn(async move {
            let snap = if android { requestbaby_core::protocols::android_snapshot(root.as_deref()) } else { requestbaby_core::protocols::web_snapshot(root.as_deref(), &env, &url, false) };
            snap.map(|s| s.elements).map_err(|e| format!("{e:#}"))
        });
        self._ui_task = Some(cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |e, cx| {
                e.ui_snapshot_loading = false;
                e.ui_snapshot = Some(res);
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// 地址栏右侧：读取元素
    pub(super) fn render_snapshot_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let android = self.protocol == Protocol::Android;
        let n = self.ui_snapshot.as_ref().and_then(|r| r.as_ref().ok()).map(|e| e.len());
        let label = if self.ui_snapshot_loading {
            "读取中…".to_string()
        } else {
            match n {
                Some(n) => format!("元素 {n}"),
                None => "读取元素".to_string(),
            }
        };
        let tip = if android {
            "读取设备当前界面的元素与推荐定位，步骤里点「元素」即可选用（等价命令：requestbaby android snapshot）"
        } else {
            "打开这个页面，读取按钮 / 链接 / 输入框与推荐定位，步骤里点「元素」即可选用（等价命令：requestbaby web snapshot <地址>）"
        };
        Button::new("ui-snapshot")
            .small()
            .outline()
            .icon(IconName::Search)
            .label(label)
            .tooltip(tip)
            .disabled(self.ui_snapshot_loading)
            .on_click(cx.listener(|this, _, _, cx| this.load_ui_snapshot(cx)))
            .into_any_element()
    }

    pub(super) fn render_ui_parts(&self, cx: &mut Context<Self>) -> AnyElement {
        let android = self.protocol == Protocol::Android;
        let theme = cx.theme();
        let (muted, success, warning) = (theme.muted_foreground, theme.success, theme.warning);
        let n = self.ui_steps.len();
        let tabs = TabBar::new("ui-parts")
            .underline()
            .small()
            .selected_index(if android { self.ui_tab } else { 0 })
            .child(Tab::new().label("步骤").when(n > 0, |t| t.suffix(div().text_xs().text_color(success).child(format!("{n}")))))
            .when(android, |t| t.child(Tab::new().label("应用")))
            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                this.ui_tab = *ix;
                cx.notify();
            }));
        let body = if android && self.ui_tab == 1 {
            let hint = |t: &'static str| div().text_xs().text_color(muted).child(t);
            v_flex()
                .gap_3()
                .child(v_flex().gap_1().child(div().text_xs().font_weight(FontWeight::MEDIUM).child("启动的 Activity")).child(Input::new(&self.ui_activity).small()).child(hint("缺省用应用的启动入口")))
                .child(v_flex().gap_1().child(div().text_xs().font_weight(FontWeight::MEDIUM).child("先安装 APK")).child(Input::new(&self.ui_apk).small()).child(hint("相对项目根或绝对路径；一次运行内只装一次")))
                .child(
                    Switch::new("ui-clear-data")
                        .small()
                        .label("启动前清除应用数据（相当于全新安装）")
                        .checked(self.ui_clear_data)
                        .on_change(cx.listener(|this, v: &bool, _, cx| {
                            this.ui_clear_data = *v;
                            this.refresh_dirty(cx);
                            cx.notify();
                        })),
                )
                .into_any_element()
        } else {
            self.render_ui_steps(cx)
        };
        let snapshot_error = match &self.ui_snapshot {
            Some(Err(e)) => Some(div().text_xs().text_color(warning).child(format!("读取元素失败：{e}"))),
            _ => None,
        };
        v_flex().gap_2().child(tabs).children(snapshot_error).child(body).into_any_element()
    }

    fn render_ui_steps(&self, cx: &mut Context<Self>) -> AnyElement {
        let android = self.protocol == Protocol::Android;
        let theme = cx.theme();
        let (muted, border, radius, danger) = (theme.muted_foreground, theme.border, theme.radius, theme.danger);
        let row_border = theme.table_row_border;
        let mono = theme.mono_font_family.clone();
        let view = cx.entity().downgrade();
        let n = self.ui_steps.len();
        let actions: Vec<UiAction> = if android { UiAction::ANDROID.to_vec() } else { UiAction::WEB.to_vec() };
        let elements: Vec<SnapElement> = self.ui_snapshot.as_ref().and_then(|r| r.as_ref().ok()).cloned().unwrap_or_default();
        let forms = self.current_ui_steps(cx);
        let rows = self.ui_steps.iter().zip(forms).enumerate().map(|(ix, (r, form))| {
            let current = r.action;
            let (v1, v2) = (view.clone(), view.clone());
            let acts = actions.clone();
            let els = elements.clone();
            // 人话复述这一步（写错时显示原因）
            let (summary, bad) = match form.to_value(ix + 1, android).and_then(|v| serde_json::from_value::<requestbaby_core::config::UiStep>(v).map_err(|e| e.to_string())) {
                Ok(step) => (requestbaby_core::runner::ui::describe_ui_step(&step, android), false),
                Err(e) => (e, true),
            };
            let line1 = h_flex()
                .gap_2()
                .child(div().w(px(22.)).text_xs().font_family(mono.clone()).text_color(muted).child(format!("{}", ix + 1)))
                .child(
                    Button::new(("ui-action", ix))
                        .small()
                        .outline()
                        .w(px(84.))
                        .label(if android && current == UiAction::Click { "点按" } else { current.label() })
                        .dropdown_menu(move |menu, _, _| {
                            let mut menu = menu;
                            for a in &acts {
                                let (v, a) = (v1.clone(), *a);
                                let label = if android && a == UiAction::Click { "点按" } else { a.label() };
                                menu = menu.item(PopupMenuItem::new(label).checked(a == current).on_click(move |_, window, cx| {
                                    let _ = v.update(cx, |e, cx| e.set_ui_action(ix, a, window, cx));
                                }));
                            }
                            menu
                        }),
                )
                .when(ui_targets_locator(current) || ui_has_text(current), |this| this.child(div().flex_1().min_w_0().child(Input::new(&r.target).small())))
                .when(!ui_targets_locator(current) && !ui_has_text(current), |this| this.child(div().flex_1()))
                .when(ui_targets_locator(current), |this| {
                    let tip = if els.is_empty() { "先点地址栏右侧「读取元素」".to_string() } else { format!("从读取到的 {} 个元素里选", els.len()) };
                    this.child(
                        Button::new(("ui-pick", ix)).small().ghost().label("元素").tooltip(tip).disabled(els.is_empty()).dropdown_menu(move |menu, _, _| {
                            let mut menu = menu.max_h(px(420.));
                            for e in &els {
                                let (v, loc) = (v2.clone(), e.locator.clone());
                                let name: String = e.name.chars().take(30).collect();
                                let state = if e.states.is_empty() { String::new() } else { format!("  [{}]", e.states.join(" ")) };
                                menu = menu.item(PopupMenuItem::new(format!("{}「{name}」{state}", e.kind)).on_click(move |_, window, cx| {
                                    let _ = v.update(cx, |ed, cx| ed.pick_element(ix, loc.clone(), window, cx));
                                }));
                            }
                            menu
                        }),
                    )
                })
                .when(matches!(current, UiAction::Fill | UiAction::Select), |this| this.child(div().w(px(160.)).child(Input::new(&r.value).small())))
                .when(current == UiAction::Launch, |this| {
                    this.child(Button::new(("ui-clear", ix)).xsmall().ghost().label("清除数据").selected(r.clear_data).tooltip("启动前清除应用数据").on_click(cx.listener(move |this, _, window, cx| this.toggle_ui_flag(ix, true, window, cx))))
                })
                .when(ui_targets_locator(current), |this| {
                    this.child(
                        Button::new(("ui-optional", ix))
                            .xsmall()
                            .ghost()
                            .label("可选")
                            .selected(r.optional)
                            .tooltip("找不到目标时跳过这一步而不失败（如可能出现的弹窗）")
                            .on_click(cx.listener(move |this, _, window, cx| this.toggle_ui_flag(ix, false, window, cx))),
                    )
                })
                .child(
                    h_flex()
                        .child(Button::new(("ui-up", ix)).xsmall().ghost().icon(IconName::ArrowUp).disabled(ix == 0).on_click(cx.listener(move |this, _, window, cx| this.move_ui_step(ix, true, window, cx))))
                        .child(Button::new(("ui-down", ix)).xsmall().ghost().icon(IconName::ArrowDown).disabled(ix + 1 == n).on_click(cx.listener(move |this, _, window, cx| this.move_ui_step(ix, false, window, cx))))
                        .child(Button::new(("ui-del", ix)).xsmall().ghost().icon(IconName::Close).on_click(cx.listener(move |this, _, window, cx| this.remove_ui_step(ix, window, cx)))),
                );
            let line2 = h_flex()
                .gap_2()
                .pl(px(30.))
                .child(div().flex_1().min_w_0().text_xs().text_color(if bad { danger } else { muted }).overflow_hidden().text_ellipsis().whitespace_nowrap().child(summary))
                .child(
                    div()
                        .id(("ui-capture", ix))
                        .w(px(220.))
                        .child(Input::new(&r.capture).xsmall())
                        .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("本步完成后取值存为变量：{\"orderNo\": {\"testId\": \"order-no\"}} / {\"page\": \"url\"}").build(window, cx)),
                )
                .when(current != UiAction::Wait, |this| this.child(div().w(px(70.)).child(Input::new(&r.timeout).xsmall())));
            v_flex().id(("ui-step", ix)).gap_1().py_1().border_b_1().border_color(row_border).child(line1).child(line2)
        });
        let add = |a: UiAction, cx: &mut Context<Self>| {
            let label = if android && a == UiAction::Click { "点按" } else { a.label() };
            Button::new(("ui-add", a as usize)).xsmall().outline().icon(IconName::Plus).label(label).on_click(cx.listener(move |this, _, window, cx| {
                this.edit_ui_steps(|s| s.push(UiStepForm::new(a)), window, cx);
            }))
        };
        let quick: Vec<UiAction> = if android {
            vec![UiAction::Click, UiAction::Fill, UiAction::Expect, UiAction::WaitFor, UiAction::Swipe, UiAction::Back, UiAction::Press, UiAction::Screenshot]
        } else {
            vec![UiAction::Click, UiAction::Fill, UiAction::Expect, UiAction::WaitFor, UiAction::Select, UiAction::Press, UiAction::Goto, UiAction::Screenshot]
        };
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(div().text_xs().font_weight(FontWeight::MEDIUM).child("操作步骤"))
                    .child(div().text_xs().text_color(muted).child(if android {
                        "启动应用后按顺序执行；每一步自动等待目标出现、自动截图留证；任一步失败即停止并保存现场"
                    } else {
                        "打开页面后按顺序执行；每一步自动等待目标出现、可点击，自动截图留证；任一步失败即停止并保存现场"
                    })),
            )
            .child(if self.ui_steps.is_empty() {
                div()
                    .h(px(72.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(radius)
                    .border_1()
                    .border_dashed()
                    .border_color(border)
                    .text_sm()
                    .text_color(muted)
                    .child(if android { "没有步骤：只验证应用能启动（留下启动后的截图）" } else { "没有步骤：只验证页面能打开（留下打开后的截图）" })
                    .into_any_element()
            } else {
                v_flex().children(rows).into_any_element()
            })
            .child(h_flex().gap_1().flex_wrap().children(quick.into_iter().map(|a| add(a, cx))).child({
                let more: Vec<UiAction> = actions.iter().copied().filter(|a| !matches!(a, UiAction::Click | UiAction::Fill | UiAction::Expect | UiAction::WaitFor | UiAction::Screenshot)).collect();
                let v = view.clone();
                Button::new("ui-add-more").xsmall().ghost().icon(IconName::ChevronDown).label("更多").dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    for a in &more {
                        let (v, a) = (v.clone(), *a);
                        menu = menu.item(PopupMenuItem::new(a.label()).on_click(move |_, window, cx| {
                            let _ = v.update(cx, |e, cx| e.edit_ui_steps(|s| s.push(UiStepForm::new(a)), window, cx));
                        }));
                    }
                    menu
                })
            }))
            .into_any_element()
    }
}
