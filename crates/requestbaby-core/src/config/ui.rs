//! 界面自动化：`web`（浏览器，Chrome DevTools 协议）与 `android`（adb + uiautomator）两种「协议」。
//!
//! 和 WebSocket 一样一步一个动作，两端共用同一套步骤、定位与断言写法：
//! - 定位按用户看得到的东西写：角色 + 名称（`{"role": "button", "name": "登录"}`）、表单标签、可见文字；
//!   Android 用 `id`（resource-id）/ `text` / `desc`（无障碍描述）。CSS / testId 只作兜底
//! - 每一步自动等待元素出现、可见、可操作，不写 sleep
//! - 每一步自动截图留证：点击类截图标出点了哪里，断言截图标出验证到的元素；失败另存现场
//!
//! 结果映射到通用的 expect：`status` = Web 打开页面的 HTTP 状态（Android 无），
//! `body` = 结束时的页面 / 界面（Web：`{"url", "title"}`，Android：`{"package", "activity"}`）。
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 浏览器里的一个用例：打开 `url`，按 `steps` 一步步操作与断言
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WebSpec {
    /// 首先打开的页面；支持 {{var}}
    pub url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<UiStep>,
    /// 视口尺寸（缺省 1280×800，或项目 web.viewport）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewport: Option<Viewport>,
    /// 每一步等待的默认超时毫秒（缺省 10000）；打开页面用项目 timeoutMs
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// 手机上的一个用例：启动 `app`，按 `steps` 一步步操作与断言
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AndroidSpec {
    /// 应用包名，如 `com.example.shop`
    pub app: String,
    /// 启动的 Activity（缺省用应用的启动入口）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    /// 先安装这个 APK（相对项目根或绝对路径）；一次运行内只装一次
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apk: Option<String>,
    /// 启动前清除应用数据（全新安装后的状态）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clear_data: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<UiStep>,
    /// 每一步等待的默认超时毫秒（缺省 10000）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
}

impl Default for Viewport {
    fn default() -> Self {
        Viewport { width: 1280, height: 800 }
    }
}

/// 找元素 + （等待 / 断言时）对它或页面的要求 + （捕获时）取哪个值。
///
/// 定位：`role`+`name` / `label` / `text` / `placeholder` / `testId` / `css`（Web）；
/// `id` / `text` / `desc`（Android）。可加 `exact`（完全相等，缺省包含即可）与 `nth`（第几个，从 0 开始）。
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Locator {
    /// 无障碍角色：button / link / textbox / checkbox / radio / combobox / heading / tab / menuitem / cell …
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// 无障碍名称（按钮文字、链接文字、输入框标签…），配合 role
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 表单控件的标签（`<label>`、aria-label，没有标签时用占位文字）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// 可见文字（包含即可）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// 输入框占位文字
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    /// `data-testid`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_id: Option<String>,
    /// CSS 选择器（兜底：依赖页面实现，改版容易失效）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub css: Option<String>,
    /// Android resource-id：写全 `com.x:id/phone` 或只写 `phone`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Android 无障碍描述（content-desc）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desc: Option<String>,
    /// 名称 / 文字完全相等（缺省包含即可，忽略大小写）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact: Option<bool>,
    /// 匹配到多个时取第几个（从 0 开始）；不写时匹配到多个即报错，避免点错
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nth: Option<usize>,

    // ---- 等待 / 断言的条件（只在 waitFor / expect 里用）----
    /// 当前地址包含（Web）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// 页面标题包含（Web）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// 当前 Activity 包含（Android）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    /// 元素状态，缺省 visible
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<UiState>,
    /// 可见的匹配元素恰好这么多个
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
    /// 输入框的值（exact 时完全相等，否则包含）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// 元素的文字包含
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_text: Option<String>,

    // ---- 捕获（只在 capture 里用）----
    /// 取元素的属性（如 href），缺省取文字（输入框取值）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attr: Option<String>,
    /// 用正则从取到的文字里截取（有分组取第 1 组）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UiState {
    Visible,
    Hidden,
    Enabled,
    Disabled,
    Checked,
    Unchecked,
}

impl UiState {
    pub fn label(&self) -> &'static str {
        match self {
            UiState::Visible => "可见",
            UiState::Hidden => "不可见",
            UiState::Enabled => "可用",
            UiState::Disabled => "禁用",
            UiState::Checked => "已勾选",
            UiState::Unchecked => "未勾选",
        }
    }
}

/// 捕获来源：`"url"` / `"title"`（Web）/ `"activity"`（Android），或一个元素
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum CaptureSource {
    Builtin(String),
    Element(Box<Locator>),
}

/// 一个步骤：下面的动作恰好写一个；`value` / `clearData` 是动作的参数
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UiStep {
    /// 打开地址（Web）：绝对地址，或相对当前页面的路径
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goto: Option<String>,
    /// 点击元素（Android 同 tap）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub click: Option<Locator>,
    /// 点击元素（Android；Web 同 click）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tap: Option<Locator>,
    /// 长按（Android）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_press: Option<Locator>,
    /// 鼠标悬停（Web）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hover: Option<Locator>,
    /// 清空并输入 `value`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<Locator>,
    /// 下拉框选择 `value`（选项文字或值，Web）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub select: Option<Locator>,
    /// 往当前焦点输入文字（Android）
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_text: Option<String>,
    /// 按键：Enter / Tab / Escape / Backspace / ArrowDown / Control+a …；Android 还有 BACK / HOME
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub press: Option<String>,
    /// 滑动屏幕：up / down / left / right（手指移动的方向，Android）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swipe: Option<String>,
    /// 返回上一页（Web 浏览器后退 / Android 返回键）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub back: Option<bool>,
    /// （重新）启动应用（Android）；配合 `clearData`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<bool>,
    /// 关闭应用（Android）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<bool>,
    /// 一直等到条件满足（不满足则失败）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_for: Option<Locator>,
    /// 断言：条件在超时内满足即通过，计入断言结果
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<Locator>,
    /// 额外截一张图并命名（每一步本来就会自动截图）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot: Option<String>,
    /// 静候毫秒数（尽量用 waitFor）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<u64>,

    /// fill / select 的值；支持 {{var}}
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// launch 前清除应用数据
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clear_data: Option<bool>,
    /// 本步完成后取值存为变量：变量名 → "url" / "title" / "activity" 或元素
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub capture: BTreeMap<String, CaptureSource>,
    /// 本步等待超时毫秒
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// 找不到目标时跳过本步而不失败（如可能出现的弹窗）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub optional: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiAction {
    Goto,
    Click,
    LongPress,
    Hover,
    Fill,
    Select,
    Type,
    Press,
    Swipe,
    Back,
    Launch,
    Stop,
    WaitFor,
    Expect,
    Screenshot,
    Wait,
}

impl UiAction {
    /// 界面上可选的动作（Web / Android 各自适用的）
    pub const WEB: [UiAction; 11] = [
        UiAction::Click,
        UiAction::Fill,
        UiAction::Select,
        UiAction::Press,
        UiAction::Hover,
        UiAction::Goto,
        UiAction::Back,
        UiAction::WaitFor,
        UiAction::Expect,
        UiAction::Screenshot,
        UiAction::Wait,
    ];
    pub const ANDROID: [UiAction; 13] = [
        UiAction::Click,
        UiAction::Fill,
        UiAction::Type,
        UiAction::Press,
        UiAction::LongPress,
        UiAction::Swipe,
        UiAction::Back,
        UiAction::Launch,
        UiAction::Stop,
        UiAction::WaitFor,
        UiAction::Expect,
        UiAction::Screenshot,
        UiAction::Wait,
    ];

    /// 写进文件的键（点击：Web 写 click，Android 写 tap）
    pub fn key(&self, android: bool) -> &'static str {
        match self {
            UiAction::Goto => "goto",
            UiAction::Click if android => "tap",
            UiAction::Click => "click",
            UiAction::LongPress => "longPress",
            UiAction::Hover => "hover",
            UiAction::Fill => "fill",
            UiAction::Select => "select",
            UiAction::Type => "type",
            UiAction::Press => "press",
            UiAction::Swipe => "swipe",
            UiAction::Back => "back",
            UiAction::Launch => "launch",
            UiAction::Stop => "stop",
            UiAction::WaitFor => "waitFor",
            UiAction::Expect => "expect",
            UiAction::Screenshot => "screenshot",
            UiAction::Wait => "wait",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            UiAction::Goto => "打开",
            UiAction::Click => "点击",
            UiAction::LongPress => "长按",
            UiAction::Hover => "悬停",
            UiAction::Fill => "填写",
            UiAction::Select => "选择",
            UiAction::Type => "输入",
            UiAction::Press => "按键",
            UiAction::Swipe => "滑动",
            UiAction::Back => "返回",
            UiAction::Launch => "启动",
            UiAction::Stop => "关闭应用",
            UiAction::WaitFor => "等待",
            UiAction::Expect => "断言",
            UiAction::Screenshot => "截图",
            UiAction::Wait => "静候",
        }
    }

    /// 目标是一个元素（需要定位）
    pub fn targets_element(&self) -> bool {
        matches!(self, UiAction::Click | UiAction::LongPress | UiAction::Hover | UiAction::Fill | UiAction::Select)
    }

    pub fn supported(&self, android: bool) -> bool {
        if android { Self::ANDROID.contains(self) } else { Self::WEB.contains(self) }
    }
}

impl UiStep {
    /// 写了哪些动作（合法步骤恰好一个）
    pub fn actions(&self) -> Vec<UiAction> {
        let mut out = vec![];
        let mut add = |cond: bool, a: UiAction| {
            if cond && !out.contains(&a) {
                out.push(a);
            }
        };
        add(self.goto.is_some(), UiAction::Goto);
        add(self.click.is_some() || self.tap.is_some(), UiAction::Click);
        add(self.long_press.is_some(), UiAction::LongPress);
        add(self.hover.is_some(), UiAction::Hover);
        add(self.fill.is_some(), UiAction::Fill);
        add(self.select.is_some(), UiAction::Select);
        add(self.type_text.is_some(), UiAction::Type);
        add(self.press.is_some(), UiAction::Press);
        add(self.swipe.is_some(), UiAction::Swipe);
        add(self.back.is_some(), UiAction::Back);
        add(self.launch.is_some(), UiAction::Launch);
        add(self.stop.is_some(), UiAction::Stop);
        add(self.wait_for.is_some(), UiAction::WaitFor);
        add(self.expect.is_some(), UiAction::Expect);
        add(self.screenshot.is_some(), UiAction::Screenshot);
        add(self.wait.is_some(), UiAction::Wait);
        // click 与 tap 同时写算两个动作
        if self.click.is_some() && self.tap.is_some() {
            out.push(UiAction::Click);
        }
        out
    }

    pub fn action(&self) -> Option<UiAction> {
        match self.actions().as_slice() {
            [one] => Some(*one),
            _ => None,
        }
    }

    /// 动作的目标元素 / 条件
    pub fn locator(&self) -> Option<&Locator> {
        self.click
            .as_ref()
            .or(self.tap.as_ref())
            .or(self.long_press.as_ref())
            .or(self.hover.as_ref())
            .or(self.fill.as_ref())
            .or(self.select.as_ref())
            .or(self.wait_for.as_ref())
            .or(self.expect.as_ref())
    }

    /// 文字内容（变量引用扫描用）
    pub fn texts(&self) -> Vec<String> {
        let mut t: Vec<String> = [&self.goto, &self.type_text, &self.press, &self.value, &self.screenshot].into_iter().flatten().cloned().collect();
        if let Some(l) = self.locator() {
            t.extend(l.texts());
        }
        for c in self.capture.values() {
            if let CaptureSource::Element(l) = c {
                t.extend(l.texts());
            }
        }
        t
    }
}

impl Locator {
    pub fn texts(&self) -> Vec<String> {
        [
            &self.role,
            &self.name,
            &self.label,
            &self.text,
            &self.placeholder,
            &self.test_id,
            &self.css,
            &self.id,
            &self.desc,
            &self.url,
            &self.title,
            &self.activity,
            &self.value,
            &self.has_text,
        ]
        .into_iter()
        .flatten()
        .cloned()
        .collect()
    }

    /// 写了定位元素的字段（否则是纯页面条件：url / title / activity）
    pub fn has_target(&self) -> bool {
        [&self.role, &self.label, &self.text, &self.placeholder, &self.test_id, &self.css, &self.id, &self.desc].iter().any(|f| f.is_some())
    }

    /// 人话描述：按钮「登录」/ 输入框「手机号」/ 文字「订单」…
    pub fn describe(&self) -> String {
        let q = |s: &str| format!("「{s}」");
        let mut d = if let Some(role) = &self.role {
            let r = role_label(role);
            match &self.name {
                Some(n) => format!("{r}{}", q(n)),
                None => r.to_string(),
            }
        } else if let Some(l) = &self.label {
            format!("输入框{}", q(l))
        } else if let Some(t) = &self.text {
            format!("文字{}", q(t))
        } else if let Some(p) = &self.placeholder {
            format!("占位为{}的输入框", q(p))
        } else if let Some(t) = &self.test_id {
            format!("testId{}", q(t))
        } else if let Some(c) = &self.css {
            format!("css{}", q(c))
        } else if let Some(i) = &self.id {
            format!("id{}", q(i))
        } else if let Some(s) = &self.desc {
            format!("描述为{}的元素", q(s))
        } else {
            String::new()
        };
        if let Some(n) = self.nth {
            d.push_str(&format!("（第 {} 个）", n + 1));
        }
        d
    }

    /// 等待 / 断言条件的人话描述
    pub fn describe_condition(&self) -> String {
        let mut parts = vec![];
        if let Some(u) = &self.url {
            parts.push(format!("地址包含「{u}」"));
        }
        if let Some(t) = &self.title {
            parts.push(format!("标题包含「{t}」"));
        }
        if let Some(a) = &self.activity {
            parts.push(format!("界面为 {a}"));
        }
        if self.has_target() {
            let target = self.describe();
            let mut what = match (self.count, self.state) {
                (Some(n), _) => format!("{target}有 {n} 个"),
                (None, Some(s)) => format!("{target}{}", s.label()),
                (None, None) if self.value.is_none() && self.has_text.is_none() => format!("{target}可见"),
                _ => target,
            };
            if let Some(v) = &self.value {
                what.push_str(&format!("的值为「{v}」"));
            }
            if let Some(t) = &self.has_text {
                what.push_str(&format!("{}文字包含「{t}」", if self.value.is_some() { "，" } else { "的" }));
            }
            parts.push(what);
        }
        parts.join("，")
    }
}

/// 无障碍角色的中文名
pub fn role_label(role: &str) -> &str {
    match role.to_ascii_lowercase().as_str() {
        "button" => "按钮",
        "link" => "链接",
        "textbox" => "输入框",
        "searchbox" => "搜索框",
        "checkbox" => "复选框",
        "radio" => "单选框",
        "combobox" => "下拉框",
        "listbox" => "列表框",
        "option" => "选项",
        "heading" => "标题",
        "tab" => "标签页",
        "menuitem" => "菜单项",
        "cell" | "gridcell" => "单元格",
        "row" => "表格行",
        "img" | "image" => "图片",
        "dialog" | "alertdialog" => "对话框",
        "listitem" => "列表项",
        "switch" => "开关",
        "slider" => "滑块",
        "alert" | "status" => "提示",
        "navigation" => "导航",
        _ => role,
    }
}

/// 项目级 Web 配置（requestbaby.json 的 `web`）
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WebConfig {
    /// 浏览器可执行文件（缺省自动查找本机 Chrome / Edge / Chromium）；环境变量 REQUESTBABY_BROWSER 优先
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser: Option<String>,
    /// 远程浏览器（Docker / 云浏览器 / 另一台机器上开了调试端口的 Chrome）：`http://主机:9222` 或 `ws(s)://…`；
    /// 环境变量 REQUESTBABY_BROWSER_URL 优先。配了就不启动本机浏览器
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// 显示浏览器窗口（缺省无界面运行；`--headed` / REQUESTBABY_HEADED=1 临时打开）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewport: Option<Viewport>,
    /// 截图：steps（每一步，缺省）/ failure（只截失败、最终画面与 screenshot 步骤）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshots: Option<ScreenshotPolicy>,
}

/// 项目级 Android 配置（requestbaby.json 的 `android`）
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AndroidConfig {
    /// adb 路径（缺省 PATH / ANDROID_HOME）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adb: Option<String>,
    /// 设备序列号（连接多台设备时必填；环境变量 ANDROID_SERIAL 优先）。
    /// 远程模拟器 / 云手机写 `主机:端口`，没连上时自动 `adb connect`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    /// 远程 adb server（`主机[:端口]`，缺省端口 5037）：设备接在另一台机器上时用，相当于 adb -H -P
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshots: Option<ScreenshotPolicy>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ScreenshotPolicy {
    #[default]
    Steps,
    Failure,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_have_exactly_one_action_and_describe_targets() {
        let s: UiStep = serde_json::from_str(r#"{"click": {"role": "button", "name": "登录"}}"#).unwrap();
        assert_eq!(s.action(), Some(UiAction::Click));
        assert_eq!(s.locator().unwrap().describe(), "按钮「登录」");
        let s: UiStep = serde_json::from_str(r#"{"tap": {"id": "phone"}}"#).unwrap();
        assert_eq!(s.action(), Some(UiAction::Click));
        let s: UiStep = serde_json::from_str(r#"{"fill": {"label": "手机号"}, "value": "{{phone}}"}"#).unwrap();
        assert_eq!((s.action(), s.texts().contains(&"{{phone}}".to_string())), (Some(UiAction::Fill), true));
        let s: UiStep = serde_json::from_str(r#"{"click": {"text": "a"}, "tap": {"text": "b"}}"#).unwrap();
        assert_eq!(s.action(), None);
        let s: UiStep = serde_json::from_str(r#"{"type": "hello"}"#).unwrap();
        assert_eq!(s.action(), Some(UiAction::Type));
        let s: UiStep = serde_json::from_str(r#"{"expect": {"url": "/orders", "text": "订单", "state": "hidden"}}"#).unwrap();
        assert_eq!(s.expect.unwrap().describe_condition(), "地址包含「/orders」，文字「订单」不可见");
        assert!(serde_json::from_str::<UiStep>(r#"{"click": {"rol": "button"}}"#).is_err());
        let s: UiStep = serde_json::from_str(r#"{"wait": 300, "capture": {"u": "url", "no": {"text": "订单号", "regex": "ORD-\\d+"}}}"#).unwrap();
        assert!(matches!(&s.capture["u"], CaptureSource::Builtin(b) if b == "url"));
        assert!(matches!(&s.capture["no"], CaptureSource::Element(l) if l.regex.is_some()));
    }
}
