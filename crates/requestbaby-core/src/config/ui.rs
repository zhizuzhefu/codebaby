//! 界面自动化：`web`（浏览器，Chrome DevTools 协议）、`android`（adb + uiautomator）与 `desktop`（Mac 桌面应用，agent-desktop 无障碍树）三种「协议」。
//!
//! 和 WebSocket 一样一步一个动作，两端共用同一套步骤、定位与断言写法：
//! - 定位按用户看得到的东西写：角色 + 名称（`{"role": "button", "name": "登录"}`）、表单标签、可见文字；
//!   Android 用 `id`（resource-id）/ `text` / `desc`（无障碍描述）；Mac 桌面用 role + name（无障碍角色与名称）/ `id`（AXIdentifier）。CSS / testId 只作兜底
//! - 每一步自动等待元素出现、可见、可操作，不写 sleep
//! - 每一步自动截图留证：点击类截图标出点了哪里，断言截图标出验证到的元素；失败另存现场
//! - 界面结构里没有文字的页面（微信小程序、自绘控件…）用 `ocr` 按屏幕上看到的文字定位：对截图做文字识别，三端通用
//!
//! 结果映射到通用的 expect：`status` = Web 打开页面的 HTTP 状态（Android 无），
//! `body` = 结束时的页面 / 界面（Web：`{"url", "title"}`，Android：`{"package", "activity"}`，Mac 桌面：`{"app", "window"}`）。
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
    /// 用例失败时发的通知（只能写 `notify`）。`{{error}}` 是失败原因，`{{failedStep}}` 是出错的那一步
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_fail: Vec<UiStep>,
    /// 视口尺寸（缺省 1280×800，或项目 web.viewport）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewport: Option<Viewport>,
    /// 每一步等待的默认超时毫秒（缺省 10000）；打开页面用项目 timeoutMs
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "timeout", deserialize_with = "de_ms_opt")]
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
    /// 接管设备上当前的界面：不启动、不关闭应用，从它现在停着的页面开始（微信小程序等要先手动走到目标页的场景）；
    /// 要求 `app` 已经在前台，不能和 activity / apk / clearData 同时写
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attach: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<UiStep>,
    /// 用例失败时发的通知（只能写 `notify`）。`{{error}}` 是失败原因，`{{failedStep}}` 是出错的那一步
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_fail: Vec<UiStep>,
    /// 每一步等待的默认超时毫秒（缺省 10000）
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "timeout", deserialize_with = "de_ms_opt")]
    pub timeout_ms: Option<u64>,
}

/// Mac 桌面应用里的一个用例：启动（或接管）`app`，按 `steps` 一步步操作与断言
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DesktopSpec {
    /// 应用名称（如 `TextEdit`）或 bundle ID（如 `com.apple.TextEdit`）
    pub app: String,
    /// 窗口标题包含这段文字的窗口（应用有多个窗口时用；缺省取唯一可见窗口）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    /// 开始前先退出应用再重新启动（缺省接管已在运行的应用，不动它）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<UiStep>,
    /// 用例失败时发的通知（只能写 `notify`）。`{{error}}` 是失败原因，`{{failedStep}}` 是出错的那一步
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_fail: Vec<UiStep>,
    /// 每一步等待的默认超时毫秒（缺省 10000）
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "timeout", deserialize_with = "de_ms_opt")]
    pub timeout_ms: Option<u64>,
}

/// 界面驱动的种类：步骤循环按它决定轮询节奏、可用动作与文案
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UiKind {
    Web,
    Android,
    Desktop,
}

impl UiKind {
    /// 界面与报错里的名字
    pub fn label(&self) -> &'static str {
        match self {
            UiKind::Web => "Web",
            UiKind::Android => "Android",
            UiKind::Desktop => "Mac 桌面",
        }
    }

    /// 用例 JSON 里的请求键
    pub fn key(&self) -> &'static str {
        match self {
            UiKind::Web => "web",
            UiKind::Android => "android",
            UiKind::Desktop => "desktop",
        }
    }

    /// 每次轮询之间的间隔：读界面结构要跑一次 adb / agent-desktop，比浏览器慢
    pub fn poll(&self) -> std::time::Duration {
        std::time::Duration::from_millis(if *self == UiKind::Web { 100 } else { 250 })
    }

    /// 步骤里可选的动作
    pub fn actions(&self) -> &'static [UiAction] {
        match self {
            UiKind::Web => &UiAction::WEB,
            UiKind::Android => &UiAction::ANDROID,
            UiKind::Desktop => &UiAction::DESKTOP,
        }
    }
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
/// `id` / `text` / `desc`（Android）；`ocr`（按屏幕上识别到的文字，三端通用）。可加 `exact`（完全相等，缺省包含即可）与 `nth`（第几个，从 0 开始）。
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Locator {
    /// 无障碍角色：button / link / textbox / checkbox / radio / combobox / heading / tab / menuitem / cell …
    /// （Mac 桌面用系统的角色名：button / textfield / checkbox / radiobutton / combobox / menubutton / treeitem …）
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
    /// Android resource-id：写全 `com.x:id/phone` 或只写 `phone`；Mac 桌面写 AXIdentifier
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Android 无障碍描述（content-desc）/ Mac 桌面的无障碍描述（AXDescription）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desc: Option<String>,
    /// 名称 / 文字完全相等（缺省包含即可，忽略大小写）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact: Option<bool>,
    /// 匹配到多个时取第几个（从 0 开始）；不写时匹配到多个即报错，避免点错
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nth: Option<usize>,

    // ---- 屏幕文字识别（OCR）：对截图识别文字，按看到的字定位，不依赖界面结构 ----
    /// 屏幕上看到的文字（忽略空白、全角半角、大小写，包含即可；`exact` 时整行相等）。写 `"*"` = 区域内的全部文字
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ocr: Option<String>,
    /// 只识别 / 查找屏幕的这一块：x / y / w / h 取 0~1，是占整张截图的比例，与分辨率无关
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<RegionRef>,
    /// 只取和这行文字在同一横排的结果（卡片、表格里「某某那一行的『已约满』」）；这行文字要在 region 之内
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row: Option<String>,
    /// 容错：允许识别结果与期望的文字相差几个字（缺省 0，必须一致）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fuzzy: Option<u32>,
    /// 识别置信度低于这个值（0~1）的文字忽略（缺省 0.3）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_confidence: Option<f64>,

    // ---- 等待 / 断言的条件（只在 waitFor / expect 里用）----
    /// 当前地址包含（Web）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// 页面标题包含（Web）/ 窗口标题包含（Mac 桌面）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// 当前 Activity 包含（Android）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    /// 元素状态，缺省 visible
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<UiState>,
    /// 可见的匹配元素的个数：数字 = 恰好这么多个；也可以写比较 `{"gt": 0}` / `{"gte": 1, "lt": 5}`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<Count>,
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

/// `region` 的写法：直接写 `{"x","y","w","h"}`，或写用例 `regions` 里起的名字（同一块区域在很多步骤里反复用时）
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum RegionRef {
    Name(String),
    Rect(Region),
}

impl RegionRef {
    /// 取出具体区域；写的是名字但没解析过（用例里没有定义）时报错
    pub fn rect(&self) -> Result<Region, String> {
        match self {
            RegionRef::Rect(r) => Ok(*r),
            RegionRef::Name(n) => Err(format!("区域「{n}」没有定义：在用例顶层 regions 里写 \"{n}\": {{ x, y, w, h }}")),
        }
    }
}

impl From<Region> for RegionRef {
    fn from(r: Region) -> Self {
        RegionRef::Rect(r)
    }
}

/// 屏幕上的一块区域：x / y / w / h 都是 0~1 的比例（相对整张截图，左上角为原点），与分辨率无关
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Region {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Region {
    /// 写法是否合法：都在 0~1 之内、宽高大于 0；不合法返回原因
    pub fn check(&self) -> Result<(), String> {
        let vals = [self.x, self.y, self.w, self.h];
        if vals.iter().any(|v| !v.is_finite() || !(0.0..=1.0).contains(v)) {
            return Err("x / y / w / h 要在 0~1 之间（占整张截图的比例）".into());
        }
        if self.w <= 0.0 || self.h <= 0.0 {
            return Err("w / h 要大于 0".into());
        }
        if self.x + self.w > 1.0 + 1e-9 || self.y + self.h > 1.0 + 1e-9 {
            return Err("区域超出了屏幕（x + w、y + h 不能大于 1）".into());
        }
        Ok(())
    }

    /// 换算成截图上的像素矩形 (x, y, 宽, 高)（取整并限制在图内）；区域为空返回 None
    pub fn to_pixels(&self, width: u32, height: u32) -> Option<(u32, u32, u32, u32)> {
        let (fw, fh) = (width as f64, height as f64);
        let x0 = (self.x.clamp(0.0, 1.0) * fw).floor();
        let y0 = (self.y.clamp(0.0, 1.0) * fh).floor();
        let x1 = ((self.x + self.w).clamp(0.0, 1.0) * fw).ceil();
        let y1 = ((self.y + self.h).clamp(0.0, 1.0) * fh).ceil();
        if x1 - x0 < 1.0 || y1 - y0 < 1.0 {
            return None;
        }
        Some((x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32))
    }
}

/// 个数要求：数字 = 恰好；或比较（与接口断言同一套运算符名：gt / gte / lt / lte）
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Count {
    Exact(usize),
    Cmp(CountCmp),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CountCmp {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gt: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gte: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lt: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lte: Option<usize>,
}

impl Count {
    pub fn matches(&self, n: usize) -> bool {
        match self {
            Count::Exact(k) => n == *k,
            Count::Cmp(c) => c.gt.is_none_or(|k| n > k) && c.gte.is_none_or(|k| n >= k) && c.lt.is_none_or(|k| n < k) && c.lte.is_none_or(|k| n <= k),
        }
    }

    /// 一条比较都没写（`{}`）
    pub fn is_empty(&self) -> bool {
        matches!(self, Count::Cmp(c) if c.gt.is_none() && c.gte.is_none() && c.lt.is_none() && c.lte.is_none())
    }

    /// 人话：「5 个」「多于 0 个」「至少 1 个且少于 5 个」
    pub fn describe(&self) -> String {
        match self {
            Count::Exact(n) => format!("{n} 个"),
            Count::Cmp(c) => {
                let mut parts = vec![];
                if let Some(k) = c.gt {
                    parts.push(format!("多于 {k} 个"));
                }
                if let Some(k) = c.gte {
                    parts.push(format!("至少 {k} 个"));
                }
                if let Some(k) = c.lt {
                    parts.push(format!("少于 {k} 个"));
                }
                if let Some(k) = c.lte {
                    parts.push(format!("至多 {k} 个"));
                }
                parts.join("且")
            }
        }
    }
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

/// 时长文字 → 毫秒：`500ms` / `2.5s` / `3s` / `1m` / `8h` / `1d`，可连写 `1h30m`；纯数字当毫秒；`forever` = 不限时（只给 repeat 的 `for` 用）
pub fn parse_duration_ms(text: &str) -> Result<u64, String> {
    let t = text.trim().to_ascii_lowercase();
    if t.is_empty() {
        return Err("时长是空的".into());
    }
    if matches!(t.as_str(), "forever" | "永远" | "不限") {
        return Ok(FOREVER_MS);
    }
    if let Ok(n) = t.parse::<u64>() {
        return Ok(n);
    }
    let bad = || format!("看不懂时长「{text}」：写法如 500ms / 2.5s / 1m / 8h / 1h30m");
    let (mut total, mut rest) = (0f64, t.as_str());
    while !rest.is_empty() {
        let num_end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.')).ok_or_else(bad)?;
        let n: f64 = rest[..num_end].parse().map_err(|_| bad())?;
        rest = &rest[num_end..];
        let (unit_ms, len) = if rest.starts_with("ms") {
            (1.0, 2)
        } else if rest.starts_with('s') {
            (1000.0, 1)
        } else if rest.starts_with('m') {
            (60_000.0, 1)
        } else if rest.starts_with('h') {
            (3_600_000.0, 1)
        } else if rest.starts_with('d') {
            (86_400_000.0, 1)
        } else {
            return Err(bad());
        };
        total += n * unit_ms;
        rest = &rest[len..];
    }
    Ok(total.round() as u64)
}

/// 毫秒 → 最短的人话写法（`parse_duration_ms` 的反向）：`28800000` → `8h`，`2500` → `2.5s`，`90000` → `90s`，`1500000` → `25m`
pub fn format_duration_ms(ms: u64) -> String {
    const SECOND: u64 = 1000;
    match ms {
        0 => "0s".into(),
        n if n < SECOND => format!("{n}ms"),
        n if n % 86_400_000 == 0 => format!("{}d", n / 86_400_000),
        n if n % 3_600_000 == 0 => format!("{}h", n / 3_600_000),
        n if n % 60_000 == 0 && n >= 120_000 => format!("{}m", n / 60_000),
        n if n % SECOND == 0 => format!("{}s", n / SECOND),
        n if n % 100 == 0 => format!("{}.{}s", n / SECOND, n % SECOND / 100),
        n => format!("{n}ms"),
    }
}

/// 「不限时」：`repeat` 的 `for: "forever"`
pub const FOREVER_MS: u64 = u64::MAX;

/// 随机区间文字：`3s-8s` / `3-8s`（前面没写单位就沿用后面的）/ `500ms~2s`
fn parse_wait_range(text: &str) -> Option<Result<WaitRange, String>> {
    let sep = text.find(['~', '～', '–', '—']).or_else(|| text.find('-'))?;
    let (a, b) = (text[..sep].trim(), text[sep..].trim_start_matches(['~', '～', '–', '—', '-']).trim());
    if a.is_empty() || b.is_empty() {
        return None;
    }
    // 前一个只有数字时借用后一个的单位
    let unit: String = b.chars().skip_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let a_text = if a.chars().all(|c| c.is_ascii_digit() || c == '.') && !a.is_empty() { format!("{a}{unit}") } else { a.to_string() };
    Some((|| Ok(WaitRange { min: parse_duration_ms(&a_text)?, max: parse_duration_ms(b)? }))())
}

/// `timeout` / `for` 这类字段：数字 = 毫秒，字符串 = 时长文字
fn de_ms_opt<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Ms(u64),
        Text(String),
    }
    match Option::<Raw>::deserialize(d)? {
        None => Ok(None),
        Some(Raw::Ms(n)) => Ok(Some(n)),
        Some(Raw::Text(t)) => parse_duration_ms(&t).map(Some).map_err(serde::de::Error::custom),
    }
}

/// 静候时长：毫秒数、`"2.5s"` / `"3s-8s"` 这样的文字，或 `{"min", "max"}` 随机区间（含两端）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "WaitRaw")]
#[serde(untagged)]
pub enum Wait {
    Ms(u64),
    Range(WaitRange),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum WaitRaw {
    Ms(u64),
    Range(WaitRange),
    Text(String),
}

impl TryFrom<WaitRaw> for Wait {
    type Error = String;
    fn try_from(r: WaitRaw) -> Result<Self, String> {
        match r {
            WaitRaw::Ms(n) => Ok(Wait::Ms(n)),
            WaitRaw::Range(r) => Ok(Wait::Range(r)),
            WaitRaw::Text(t) => match parse_wait_range(&t) {
                Some(r) => r.map(Wait::Range),
                None => parse_duration_ms(&t).map(Wait::Ms),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WaitRange {
    pub min: u64,
    pub max: u64,
}

impl Wait {
    /// 本次要等多久（毫秒）：固定值原样返回，区间内随机取
    pub fn pick(&self) -> u64 {
        match self {
            Wait::Ms(ms) => *ms,
            Wait::Range(r) => {
                let (lo, hi) = (r.min.min(r.max), r.min.max(r.max));
                lo + random_u64() % (hi - lo + 1)
            }
        }
    }

    /// 人话：`500 ms` / `3000~8000 ms（随机）`
    pub fn describe(&self) -> String {
        match self {
            Wait::Ms(ms) => format!("{ms} ms"),
            Wait::Range(r) => format!("{}~{} ms（随机）", r.min.min(r.max), r.min.max(r.max)),
        }
    }

    /// 写法问题：区间 min 不能大于 max
    pub fn check(&self) -> Result<(), String> {
        match self {
            Wait::Range(r) if r.min > r.max => Err(format!("wait 的 min（{}）不能大于 max（{}）", r.min, r.max)),
            _ => Ok(()),
        }
    }
}

/// 不需要密码学强度的随机数（等待抖动）：时间 + 计数器 + 进程号，经 splitmix64 混合
fn random_u64() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    let mut z = t ^ COUNTER.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed) ^ (std::process::id() as u64).rotate_left(32);
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// 重复一组步骤。`times` 与 `forMs` 至少写一个（先到的结束）；`until` 在每轮结束后判断一次，满足就提前结束
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Repeat {
    /// 最多重复几次
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub times: Option<u32>,
    /// 最长重复多久（毫秒；写 `for` 可以用 `"8h"` / `"30m"` / `"forever"`）：一轮结束时已超过就不再开始下一轮
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "for", deserialize_with = "de_ms_opt")]
    pub for_ms: Option<u64>,
    /// 轮换取值：每一轮取下一个值，到头了从头再来。`{"date": ["11-28", "11-29"]}` → 步骤里写 `{{date}}`；
    /// 只写 `cycle`（不写 times / for）就是每个值各来一轮
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cycle: BTreeMap<String, Vec<String>>,
    /// 每轮结束后判断的条件（写法同 expect，只判断一次、不等待）：满足就提前结束
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<Locator>,
    /// 轮次变量名（第几轮，从 1 开始），步骤里写 `{{i}}` 引用；缺省 `i`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub var: Option<String>,
    /// 每一轮执行的步骤
    #[serde(default)]
    pub steps: Vec<UiStep>,
}

impl Repeat {
    pub const MAX_DEPTH: usize = 4;

    pub fn var_name(&self) -> &str {
        self.var.as_deref().filter(|v| !v.trim().is_empty()).unwrap_or("i")
    }

    /// 最多几轮：写了 times 用它；只写了 cycle（没有 for）= 最长的那一组各来一轮
    pub fn effective_times(&self) -> Option<u32> {
        self.times.or_else(|| if self.for_ms.is_none() { self.cycle.values().map(|v| v.len() as u32).max().filter(|n| *n > 0) } else { None })
    }

    /// 第 `round` 轮（从 1 开始）各 cycle 变量取什么值
    pub fn cycle_values(&self, round: u32) -> Vec<(String, String)> {
        let idx = round.saturating_sub(1) as usize;
        self.cycle.iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| (k.clone(), v[idx % v.len()].clone())).collect()
    }
}

/// 条件分支
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IfBlock {
    /// 条件（写法同 expect / waitFor）
    pub when: Locator,
    /// 满足时执行
    #[serde(default)]
    pub then: Vec<UiStep>,
    /// 不满足时执行
    #[serde(default, rename = "else")]
    pub else_: Vec<UiStep>,
}

/// 通知：字符串 = 通知内容；对象可以再带 `webhook`（把内容 POST 到这个地址）。
/// 只做三件固定的事——弹本机通知、写 `.requestbaby/alerts.log`、POST 到 webhook——不运行任何命令
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Notify {
    Text(String),
    Spec(NotifySpec),
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NotifySpec {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook: Option<String>,
}

impl Notify {
    pub fn message(&self) -> &str {
        match self {
            Notify::Text(c) => c,
            Notify::Spec(s) => &s.message,
        }
    }

    pub fn webhook(&self) -> Option<&str> {
        match self {
            Notify::Text(_) => None,
            Notify::Spec(s) => s.webhook.as_deref(),
        }
    }
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
    /// 往当前焦点输入文字（Android / Mac 桌面）
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_text: Option<String>,
    /// 按键：Enter / Tab / Escape / Backspace / ArrowDown / Control+a …；Android 还有 BACK / HOME；Mac 桌面可写 Meta+s / cmd+s
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub press: Option<String>,
    /// 滑动屏幕：up / down / left / right（手指移动的方向，Android；Mac 桌面滚动窗口里最大的滚动区域，方向含义相同）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swipe: Option<String>,
    /// 返回上一页（Web 浏览器后退 / Android 返回键）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub back: Option<bool>,
    /// （重新）启动应用（Android / Mac 桌面）；配合 `clearData`（仅 Android）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<bool>,
    /// 关闭应用（Android / Mac 桌面）
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
    /// 静候：毫秒数，或随机区间 `{"min": 3000, "max": 8000}`（每次执行在区间内随机取一个值；尽量用 waitFor）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<Wait>,
    /// 重复执行一组步骤（轮询监控、反复刷新）：`times` 次数 / `forMs` 总时长，至少写一个；可加 `until` 条件提前结束
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<Repeat>,
    /// 条件分支：`when` 条件满足执行 `then`，否则执行 `else`（只判断一次，不等待）
    #[serde(default, rename = "if", skip_serializing_if = "Option::is_none")]
    pub if_: Option<IfBlock>,
    /// 跳出最近的一层 `repeat`（通常写在 `if` 的 `then` 里：发现目标、通知完就结束监控）
    #[serde(default, rename = "break", skip_serializing_if = "Option::is_none")]
    pub break_: Option<bool>,
    /// 发一条通知：弹本机通知、记到 `.requestbaby/alerts.log`，配了 webhook 就 POST 过去。写内容文字，或 `{"message": "…", "webhook": "https://…"}`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify: Option<Notify>,

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
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "timeout", deserialize_with = "de_ms_opt")]
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
    Repeat,
    If,
    Notify,
    Break,
}

impl UiAction {
    /// 界面上可选的动作（Web / Android 各自适用的）
    pub const WEB: [UiAction; 15] = [
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
        UiAction::Repeat,
        UiAction::If,
        UiAction::Notify,
        UiAction::Break,
    ];
    pub const DESKTOP: [UiAction; 16] = [
        UiAction::Click,
        UiAction::Fill,
        UiAction::Select,
        UiAction::Type,
        UiAction::Press,
        UiAction::Swipe,
        UiAction::Launch,
        UiAction::Stop,
        UiAction::WaitFor,
        UiAction::Expect,
        UiAction::Screenshot,
        UiAction::Wait,
        UiAction::Repeat,
        UiAction::If,
        UiAction::Notify,
        UiAction::Break,
    ];
    pub const ANDROID: [UiAction; 17] = [
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
        UiAction::Repeat,
        UiAction::If,
        UiAction::Notify,
        UiAction::Break,
    ];

    /// 写进文件的键（点击：Web 写 click，Android 写 tap）
    pub fn key(&self, kind: UiKind) -> &'static str {
        match self {
            UiAction::Goto => "goto",
            UiAction::Click if kind == UiKind::Android => "tap",
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
            UiAction::Repeat => "repeat",
            UiAction::If => "if",
            UiAction::Notify => "notify",
            UiAction::Break => "break",
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
            UiAction::Repeat => "重复",
            UiAction::If => "如果",
            UiAction::Notify => "通知",
            UiAction::Break => "跳出循环",
        }
    }

    /// 目标是一个元素（需要定位）
    pub fn targets_element(&self) -> bool {
        matches!(self, UiAction::Click | UiAction::LongPress | UiAction::Hover | UiAction::Fill | UiAction::Select)
    }

    pub fn supported(&self, kind: UiKind) -> bool {
        kind.actions().contains(self)
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
        add(self.repeat.is_some(), UiAction::Repeat);
        add(self.if_.is_some(), UiAction::If);
        add(self.notify.is_some(), UiAction::Notify);
        add(self.break_.is_some(), UiAction::Break);
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
            .or(self.if_.as_ref().map(|b| &b.when))
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
        // 重复里的子步骤和 until 各自执行时再检查（轮次变量 {{i}} 只在循环里有值）；这里只看本层的环境变量
        if let Some(n) = &self.notify {
            t.push(n.message().to_string());
            t.extend(n.webhook().map(str::to_string));
        }
        t
    }

    /// 本步自己用到的全部定位：动作目标 / 条件、repeat 的 until、capture 里的元素（不含子步骤）
    pub fn all_locators(&self) -> Vec<&Locator> {
        let mut out: Vec<&Locator> = self.locator().into_iter().collect();
        out.extend(self.repeat.as_ref().and_then(|r| r.until.as_ref()));
        for c in self.capture.values() {
            if let CaptureSource::Element(l) = c {
                out.push(l);
            }
        }
        out
    }

    /// 把 `region` 写的名字换成用例 `regions` 里定义的区域（含 repeat / if 里的子步骤）；名字没有定义的保持原样（运行时报错、validate 提前报）
    pub fn resolve_regions(&mut self, names: &BTreeMap<String, Region>) {
        let fix = |l: &mut Locator| {
            if let Some(RegionRef::Name(n)) = &l.region {
                if let Some(r) = names.get(n) {
                    l.region = Some(RegionRef::Rect(*r));
                }
            }
        };
        for l in [&mut self.click, &mut self.tap, &mut self.long_press, &mut self.hover, &mut self.fill, &mut self.select, &mut self.wait_for, &mut self.expect].into_iter().flatten() {
            fix(l);
        }
        for c in self.capture.values_mut() {
            if let CaptureSource::Element(l) = c {
                fix(l);
            }
        }
        if let Some(r) = &mut self.repeat {
            if let Some(u) = r.until.as_mut() {
                fix(u);
            }
            r.steps.iter_mut().for_each(|st| st.resolve_regions(names));
        }
        if let Some(b) = &mut self.if_ {
            fix(&mut b.when);
            b.then.iter_mut().chain(b.else_.iter_mut()).for_each(|st| st.resolve_regions(names));
        }
    }

    /// 子步骤（repeat / if 里的）：依次是 repeat.steps、if.then、if.else
    pub fn children(&self) -> Vec<&UiStep> {
        let mut out: Vec<&UiStep> = vec![];
        if let Some(r) = &self.repeat {
            out.extend(r.steps.iter());
        }
        if let Some(b) = &self.if_ {
            out.extend(b.then.iter());
            out.extend(b.else_.iter());
        }
        out
    }
}

/// 复制一份步骤，并把 `region` 里写的区域名换成用例 `regions` 里的区域（没有用到名字时原样返回）
pub fn with_regions(steps: &[UiStep], names: &BTreeMap<String, Region>) -> Vec<UiStep> {
    let mut v = steps.to_vec();
    if !names.is_empty() {
        v.iter_mut().for_each(|s| s.resolve_regions(names));
    }
    v
}

/// 把嵌套的步骤展平（含 repeat / if 里的子步骤，先父后子）：扫描「用没用到某种能力」时用
pub fn flatten_steps(steps: &[UiStep]) -> Vec<&UiStep> {
    let mut out = vec![];
    for s in steps {
        out.push(s);
        for c in s.children() {
            out.extend(flatten_steps(std::slice::from_ref(c)));
        }
    }
    out
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
            &self.ocr,
            &self.row,
        ]
        .into_iter()
        .flatten()
        .cloned()
        .collect()
    }

    /// 写了定位元素的字段（否则是纯页面条件：url / title / activity）
    pub fn has_target(&self) -> bool {
        [&self.role, &self.label, &self.text, &self.placeholder, &self.test_id, &self.css, &self.id, &self.desc, &self.ocr].iter().any(|f| f.is_some())
    }

    /// 按屏幕文字识别（OCR）定位，而不是读界面结构
    pub fn is_ocr(&self) -> bool {
        self.ocr.is_some()
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
        } else if let Some(t) = &self.ocr {
            let what = if t == "*" { "屏幕上的全部文字".to_string() } else { format!("屏幕文字{}", q(t)) };
            match &self.row {
                Some(r) => format!("和{}同一行的{what}", q(r)),
                None => what,
            }
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
                (Some(n), _) => format!("{target}有{}", if matches!(n, Count::Exact(_)) { format!(" {}", n.describe()) } else { n.describe() }),
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
        "menubutton" => "菜单按钮",
        "textfield" => "输入框",
        "statictext" => "文字",
        "radiobutton" => "单选框",
        "treeitem" => "树节点",
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

/// 项目级 Mac 桌面配置（requestbaby.json 的 `desktop`）
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DesktopConfig {
    /// agent-desktop 路径（缺省 PATH）；环境变量 REQUESTBABY_AGENT_DESKTOP 优先
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_desktop: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshots: Option<ScreenshotPolicy>,
    /// 允许用真实鼠标：按屏幕文字（`ocr`）定位的点击需要它——会把鼠标移到目标上并点击、把应用窗口带到最前，
    /// 运行期间不要动鼠标。缺省关闭：只读（识别、断言、取值）不受影响，点击 OCR 找到的文字时报错并提示这个开关
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headed: Option<bool>,
}

/// 项目级屏幕文字识别（OCR）配置（requestbaby.json 的 `ocr`）
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OcrConfig {
    /// 识别语言（缺省 `["zh-Hans", "en-US"]`）：如 `zh-Hant` / `ja-JP` / `ko-KR`；`requestbaby doctor` 列出本机支持的语言
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
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

    #[test]
    fn ocr_locator_describes_and_parses() {
        let l: Locator = serde_json::from_str(r#"{"ocr": "已约满", "region": {"x": 0, "y": 0.1, "w": 1, "h": 0.09}, "row": "张三", "fuzzy": 1, "minConfidence": 0.5, "nth": 1}"#).unwrap();
        assert!(l.is_ocr() && l.has_target());
        assert_eq!(l.describe(), "和「张三」同一行的屏幕文字「已约满」（第 2 个）");
        assert_eq!(Locator { ocr: Some("*".into()), ..Default::default() }.describe(), "屏幕上的全部文字");
        // 变量引用扫描要覆盖 ocr / row
        let t = Locator { ocr: Some("{{a}}".into()), row: Some("{{b}}".into()), ..Default::default() }.texts();
        assert!(t.contains(&"{{a}}".to_string()) && t.contains(&"{{b}}".to_string()));
        // 区域里写了不认识的字段、或比例不在 0~1：解析 / 检查都挡住
        assert!(serde_json::from_str::<Locator>(r#"{"ocr": "a", "region": {"x": 0, "y": 0, "w": 1, "h": 1, "z": 2}}"#).is_err());
        assert!(Region { x: 0.5, y: 0.0, w: 0.6, h: 1.0 }.check().is_err());
        assert!(Region { x: 0.0, y: 0.0, w: 0.0, h: 1.0 }.check().is_err());
        assert!(Region { x: -0.1, y: 0.0, w: 0.5, h: 0.5 }.check().is_err());
        assert!(Region { x: 0.0, y: 0.1, w: 1.0, h: 0.9 }.check().is_ok());
    }

    #[test]
    fn region_converts_to_pixels() {
        let r = Region { x: 0.0, y: 0.1, w: 1.0, h: 0.09 };
        assert_eq!(r.to_pixels(1080, 2400), Some((0, 240, 1080, 216)));
        // 超出图外的部分被截掉
        assert_eq!(Region { x: 0.9, y: 0.9, w: 0.5, h: 0.5 }.to_pixels(1000, 1000), Some((900, 900, 100, 100)));
        assert_eq!(Region { x: 1.0, y: 0.0, w: 0.5, h: 0.5 }.to_pixels(100, 100), None);
    }

    #[test]
    fn durations_read_like_people_write_them() {
        for (text, ms) in [("500ms", 500), ("2.5s", 2500), ("3s", 3000), ("1m", 60_000), ("8h", 28_800_000), ("1h30m", 5_400_000), ("1m30s", 90_000), ("2500", 2500), (" 1S ", 1000)] {
            assert_eq!(parse_duration_ms(text), Ok(ms), "{text}");
        }
        assert_eq!(parse_duration_ms("forever"), Ok(FOREVER_MS));
        for bad in ["", "三秒", "s", "3x", "1.2.3s", "5 分钟"] {
            let e = parse_duration_ms(bad).unwrap_err();
            assert!(e.contains("500ms") || e.contains("空"), "{bad}: {e}");
        }
        // wait：毫秒 / 文字 / 区间文字 / 区间对象
        let w = |j: &str| serde_json::from_str::<Wait>(j).map_err(|e| e.to_string());
        assert_eq!(w("1500"), Ok(Wait::Ms(1500)));
        assert_eq!(w("\"2.5s\""), Ok(Wait::Ms(2500)));
        assert_eq!(w("\"3s-8s\""), Ok(Wait::Range(WaitRange { min: 3000, max: 8000 })));
        assert_eq!(w("\"3-8s\""), Ok(Wait::Range(WaitRange { min: 3000, max: 8000 })), "前面没写单位沿用后面的");
        assert_eq!(w("\"500ms~2s\""), Ok(Wait::Range(WaitRange { min: 500, max: 2000 })));
        assert_eq!(w("{\"min\":1,\"max\":2}"), Ok(Wait::Range(WaitRange { min: 1, max: 2 })));
        assert!(w("\"三秒\"").unwrap_err().contains("看不懂时长"));
        // repeat.for / timeout 别名
        let r: Repeat = serde_json::from_str(r#"{"for":"8h","steps":[]}"#).unwrap();
        assert_eq!(r.for_ms, Some(28_800_000));
        let r: Repeat = serde_json::from_str(r#"{"for":"forever","steps":[]}"#).unwrap();
        assert_eq!(r.for_ms, Some(FOREVER_MS));
        let st: UiStep = serde_json::from_str(r#"{"wait":1,"timeout":"15s"}"#).unwrap();
        assert_eq!(st.timeout_ms, Some(15_000));
        let st: UiStep = serde_json::from_str(r#"{"wait":1,"timeoutMs":20}"#).unwrap();
        assert_eq!(st.timeout_ms, Some(20));
    }

    #[test]
    fn cycle_takes_the_next_value_each_round_and_defaults_to_one_pass() {
        let r: Repeat = serde_json::from_str(r#"{"cycle":{"date":["a","b","c"],"who":["x","y"]},"steps":[]}"#).unwrap();
        assert_eq!(r.effective_times(), Some(3), "只写 cycle = 最长的一组各来一轮");
        assert_eq!(r.cycle_values(1), vec![("date".to_string(), "a".to_string()), ("who".to_string(), "x".to_string())]);
        assert_eq!(r.cycle_values(3), vec![("date".to_string(), "c".to_string()), ("who".to_string(), "x".to_string())], "短的一组从头再来");
        assert_eq!(r.cycle_values(4)[0].1, "a");
        // 写了 for 就由时长决定什么时候停，不再默认只走一遍
        let r: Repeat = serde_json::from_str(r#"{"cycle":{"d":["a"]},"for":"1h","steps":[]}"#).unwrap();
        assert_eq!(r.effective_times(), None);
        // 写了 times 以 times 为准
        let r: Repeat = serde_json::from_str(r#"{"cycle":{"d":["a","b"]},"times":5,"steps":[]}"#).unwrap();
        assert_eq!(r.effective_times(), Some(5));
    }

    #[test]
    fn region_names_resolve_from_the_case_regions_everywhere_in_the_steps() {
        let names: BTreeMap<String, Region> = [("条".to_string(), Region { x: 0.0, y: 0.1, w: 1.0, h: 0.1 })].into();
        let steps: Vec<UiStep> = serde_json::from_str(
            r#"[{"expect":{"ocr":"a","region":"条"}},
                {"repeat":{"times":1,"until":{"ocr":"b","region":"条"},"steps":[{"tap":{"ocr":"c","region":"条"}}]}},
                {"if":{"when":{"ocr":"d","region":"条"},"then":[{"expect":{"ocr":"e","region":"没定义"}}]}},
                {"expect":{"ocr":"f","region":{"x":0.5,"y":0.5,"w":0.1,"h":0.1}}}]"#,
        )
        .unwrap();
        let out = with_regions(&steps, &names);
        let rect = |l: &Locator| l.region.as_ref().unwrap().rect();
        let want = Region { x: 0.0, y: 0.1, w: 1.0, h: 0.1 };
        assert_eq!(rect(out[0].expect.as_ref().unwrap()), Ok(want));
        let rep = out[1].repeat.as_ref().unwrap();
        assert_eq!(rect(rep.until.as_ref().unwrap()), Ok(want));
        assert_eq!(rect(rep.steps[0].tap.as_ref().unwrap()), Ok(want));
        let blk = out[2].if_.as_ref().unwrap();
        assert_eq!(rect(&blk.when), Ok(want));
        assert!(rect(blk.then[0].expect.as_ref().unwrap()).unwrap_err().contains("没有定义"), "没定义的名字运行时要说清楚");
        assert_eq!(rect(out[3].expect.as_ref().unwrap()), Ok(Region { x: 0.5, y: 0.5, w: 0.1, h: 0.1 }));
    }

    #[test]
    fn wait_accepts_ms_or_random_range() {
        let s: UiStep = serde_json::from_str(r#"{"wait": 300}"#).unwrap();
        assert_eq!((s.action(), s.wait), (Some(UiAction::Wait), Some(Wait::Ms(300))));
        assert_eq!(Wait::Ms(300).pick(), 300);
        let w: Wait = serde_json::from_str(r#"{"min": 3000, "max": 8000}"#).unwrap();
        assert_eq!(w.describe(), "3000~8000 ms（随机）");
        let picks: Vec<u64> = (0..200).map(|_| w.pick()).collect();
        assert!(picks.iter().all(|p| (3000..=8000).contains(p)), "{picks:?}");
        assert!(picks.iter().collect::<std::collections::BTreeSet<_>>().len() > 20, "随机区间应当取到不同的值");
        // 两端相等 = 固定值；反过来写会被检查出来
        assert_eq!(Wait::Range(WaitRange { min: 5, max: 5 }).pick(), 5);
        assert!(Wait::Range(WaitRange { min: 9, max: 1 }).check().is_err() && w.check().is_ok());
        assert!(serde_json::from_str::<Wait>(r#"{"min": 1, "max": 2, "x": 3}"#).is_err());
        assert!(serde_json::from_str::<Wait>(r#"{"min": 1}"#).is_err());
    }

    #[test]
    fn repeat_if_notify_parse_nest_and_flatten() {
        let s: UiStep = serde_json::from_str(
            r#"{"repeat": {"forMs": 60000, "var": "n", "until": {"ocr": "有号"}, "steps": [
                {"tap": {"ocr": "11-28"}},
                {"if": {"when": {"ocr": "有号"}, "then": [{"notify": {"message": "{{n}} 有号", "webhook": "https://hooks.example.com/x"}}], "else": [{"wait": {"min": 1, "max": 2}}]}}
            ]}}"#,
        )
        .unwrap();
        assert_eq!(s.action(), Some(UiAction::Repeat));
        let r = s.repeat.as_ref().unwrap();
        assert_eq!((r.var_name(), r.times, r.for_ms), ("n", None, Some(60000)));
        assert_eq!(Repeat::default().var_name(), "i");
        // 展平：repeat、tap、if、then 里的 notify、else 里的 wait
        let flat = flatten_steps(std::slice::from_ref(&s));
        assert_eq!(flat.iter().filter_map(|x| x.action()).collect::<Vec<_>>(), [UiAction::Repeat, UiAction::Click, UiAction::If, UiAction::Notify, UiAction::Wait]);
        // 扫描 ocr 要能看到 until、if.when 里的定位
        assert!(s.all_locators().iter().any(|l| l.is_ocr()));
        assert!(flat[2].locator().is_some_and(|l| l.is_ocr()));
        // 变量引用扫描只看本层：子步骤里的 {{n}} 在循环里才有值，执行到那一步再检查
        assert!(!s.texts().iter().any(|t| t.contains("{{n}}")));
        let notify = flat[3];
        assert_eq!(notify.notify.as_ref().unwrap().message(), "{{n}} 有号");
        assert_eq!(notify.notify.as_ref().unwrap().webhook(), Some("https://hooks.example.com/x"));
        assert!(notify.texts().contains(&"{{n}} 有号".to_string()));
        // notify 可以只写文字；写了不认识的字段会被挡住；exec（运行本机命令）已经移除，不再被认识
        let e: UiStep = serde_json::from_str(r#"{"notify": "有号了"}"#).unwrap();
        assert_eq!((e.action(), e.notify.as_ref().unwrap().message(), e.notify.as_ref().unwrap().webhook()), (Some(UiAction::Notify), "有号了", None));
        assert!(serde_json::from_str::<UiStep>(r#"{"notify": {"message": "x", "command": "rm -rf /"}}"#).is_err());
        assert!(serde_json::from_str::<UiStep>(r#"{"exec": "echo hi"}"#).is_err());
        assert!(serde_json::from_str::<UiStep>(r#"{"repeat": {"times": 2, "step": []}}"#).is_err());
        assert!(serde_json::from_str::<UiStep>(r#"{"if": {"when": {"text": "a"}, "than": []}}"#).is_err());
        // 同一步写两个动作仍然只算多动作
        let two: UiStep = serde_json::from_str(r#"{"notify": "x", "wait": 1}"#).unwrap();
        assert_eq!(two.action(), None);
        // 三端都能用
        for kind in [UiKind::Web, UiKind::Android, UiKind::Desktop] {
            assert!([UiAction::Repeat, UiAction::If, UiAction::Notify].iter().all(|a| a.supported(kind)), "{kind:?}");
        }
        // 写回时字段名不变（if / else 是保留字，序列化要还原）
        let back = serde_json::to_value(&s).unwrap();
        assert!(back["repeat"]["steps"][1]["if"]["else"].is_array() && back["repeat"]["steps"][1]["if"]["then"].is_array(), "{back}");
    }

    #[test]
    fn count_accepts_number_or_comparison() {
        let n: Count = serde_json::from_str("5").unwrap();
        assert!(n.matches(5) && !n.matches(4));
        assert_eq!(n.describe(), "5 个");
        let c: Count = serde_json::from_str(r#"{"gte": 1, "lt": 5}"#).unwrap();
        assert!(!c.matches(0) && c.matches(1) && c.matches(4) && !c.matches(5));
        assert_eq!(c.describe(), "至少 1 个且少于 5 个");
        let gt: Count = serde_json::from_str(r#"{"gt": 0}"#).unwrap();
        assert!(!gt.matches(0) && gt.matches(1));
        assert!(serde_json::from_str::<Count>(r#"{"gtt": 0}"#).is_err());
        assert!(serde_json::from_str::<Count>("{}").unwrap().is_empty());
        let l: Locator = serde_json::from_str(r#"{"text": "订单", "count": {"gt": 0}}"#).unwrap();
        assert_eq!(l.describe_condition(), "文字「订单」有多于 0 个");
        let l: Locator = serde_json::from_str(r#"{"text": "订单", "count": 3}"#).unwrap();
        assert_eq!(l.describe_condition(), "文字「订单」有 3 个");
    }
}
