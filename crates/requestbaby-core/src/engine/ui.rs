//! 界面自动化的公共部分：驱动接口（Web = Chrome DevTools 协议，Android = adb，Mac 桌面 = agent-desktop）、
//! 找到的元素、证据（每一步的记录与截图、运行环境、失败现场）、截图标注。
//!
//! 步骤循环（自动等待、断言、捕获、截图留证）在 runner 里只写一遍，每种驱动只负责「找」和「做」。
use crate::config::{Locator, UiKind};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 元素位置（Web：CSS 像素，视口坐标；Android：屏幕像素；Mac 桌面：相对窗口左上角的点）
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }
}

/// 找到的一个元素（驱动按需填写；handle 是驱动内部的引用，下一次查找后失效）
#[derive(Debug, Clone, Default)]
pub struct Element {
    pub handle: usize,
    pub rect: Rect,
    pub visible: bool,
    pub enabled: bool,
    /// 复选 / 单选 / 开关；不是这类元素为 None
    pub checked: Option<bool>,
    /// 可输入（输入框 / 文本域 / 可编辑区域）
    pub editable: bool,
    /// 只读输入框
    pub readonly: bool,
    /// 可见文字
    pub text: String,
    /// 输入框的值；下拉框为选中项文字
    pub value: Option<String>,
    /// 简短描述（遮挡提示 / 失败说明用）：`<button class="primary">`
    pub tag: String,
    /// 密码框：填写的值不进步骤描述
    pub secret: bool,
    /// 来自屏幕文字识别（OCR）：没有界面结构里的节点（handle 无效），只有屏幕上的位置，驱动只能按坐标操作它
    pub visual: bool,
    /// 屏幕文字识别的置信度（0~1）；不是 OCR 找到的为 None
    pub confidence: Option<f32>,
}

/// 当前页面 / 界面
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageInfo {
    /// Web：地址；Android：应用包名；Mac 桌面：应用名称
    pub location: String,
    /// Web：标题；Android：当前 Activity；Mac 桌面：窗口标题
    pub title: String,
}

/// 一张截图：图片字节（PNG / JPEG）+ 坐标到图片像素的倍数
pub struct Shot {
    pub bytes: Vec<u8>,
    pub scale: f64,
}

/// 点击前元素检查的结果
pub enum Actionable {
    Ok(Element),
    /// 暂时不能点（位置还在动 / 被遮挡），说明原因，继续等
    NotYet(String),
}

/// 一种界面驱动：Web 页面（一个浏览器上下文里的一个标签页）、一台 Android 设备上的应用或 Mac 桌面上的一个应用
pub trait UiDriver {
    fn kind(&self) -> UiKind;
    /// 按定位找出全部匹配的元素（含不可见的，由调用方筛选）；写法有误等无法恢复的错误返回 Err
    fn find(&mut self, loc: &Locator) -> Result<Vec<Element>>;
    fn page(&mut self) -> Result<PageInfo>;
    /// 点击前的准备：滚动到可见、确认位置稳定、没有被遮挡
    fn actionable(&mut self, el: &Element) -> Result<Actionable>;
    fn click(&mut self, el: &Element) -> Result<()>;
    fn long_press(&mut self, el: &Element) -> Result<()>;
    fn hover(&mut self, el: &Element) -> Result<()>;
    fn fill(&mut self, el: &Element, value: &str) -> Result<()>;
    /// 返回选中的选项文字
    fn select(&mut self, el: &Element, value: &str) -> Result<String>;
    fn type_text(&mut self, text: &str) -> Result<()>;
    fn press(&mut self, key: &str) -> Result<()>;
    fn swipe(&mut self, direction: &str) -> Result<()>;
    fn back(&mut self) -> Result<()>;
    fn goto(&mut self, url: &str) -> Result<()>;
    fn launch(&mut self, clear_data: bool) -> Result<()>;
    /// 接管应用当前的界面（不启动、不关闭）；只有 Android 用例支持
    fn attach(&mut self) -> Result<()> {
        anyhow::bail!("{} 用例不支持 attach", self.kind().label())
    }
    fn stop(&mut self) -> Result<()>;
    /// 动作之后等界面稳定（页面跳转加载完成 / 动画结束），最多等 timeout
    fn settle(&mut self, timeout: Duration);
    fn screenshot(&mut self) -> Result<Shot>;
    /// 屏幕文字识别（OCR）用的截图：驱动需要先同步窗口位置之类时覆盖，缺省就是普通截图
    fn screenshot_for_ocr(&mut self) -> Result<Shot> {
        self.screenshot()
    }
    /// 元素属性（Web：DOM 属性；Android / Mac 桌面：节点属性）
    fn attr(&mut self, el: &Element, name: &str) -> Result<Option<String>>;
    /// 失败现场：页面结构 / 控制台 / 网络 / logcat 等，写进 dir，返回附件清单
    fn failure_scene(&mut self, dir: &Path) -> Vec<Artifact>;
    /// 运行环境：浏览器版本 / 视口 / 设备型号 / 系统版本 / 应用版本…
    fn environment(&mut self) -> Vec<(String, String)>;
    /// 运行过程中的提示（自动接受的对话框等），取走即清空
    fn take_notes(&mut self) -> Vec<String> {
        vec![]
    }
}

// ---------------------------------------------------------------------------------------------
// 证据
// ---------------------------------------------------------------------------------------------

/// 一个界面用例的证据：每一步做了什么、结果、截图；运行环境；失败现场
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiEvidence {
    /// 证据目录（相对项目根；不在项目里运行时为绝对路径）
    pub dir: String,
    pub steps: Vec<StepRecord>,
    /// 运行环境（浏览器 / 设备 / 视口…），按展示顺序
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment: Vec<(String, String)>,
    /// 失败现场等附件
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<Artifact>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UiStepStatus {
    Passed,
    Failed,
    /// optional 步骤没找到目标，跳过
    Skipped,
}

/// 一步的记录
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepRecord {
    /// 第几步（从 1 起）；0 = 打开页面 / 启动应用，最后的「结束画面」为步数 + 1
    pub index: usize,
    /// 动作键：goto / click / fill / expect … / open / final
    pub action: String,
    /// 人话描述：点击 按钮「登录」
    pub title: String,
    pub status: UiStepStatus,
    /// 距用例开始的毫秒数
    pub at_ms: u64,
    pub duration_ms: u64,
    /// 截图文件名（在证据目录里）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot: Option<String>,
    /// 失败原因 / 补充说明（选中的选项、捕获的值、被遮挡…）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// 附件：失败现场的页面结构、控制台、网络、logcat…
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    pub label: String,
    /// 文件名（在证据目录里）
    pub file: String,
}

impl UiEvidence {
    /// 证据目录的绝对路径（root = 项目根）
    pub fn abs_dir(&self, root: Option<&Path>) -> PathBuf {
        let d = PathBuf::from(&self.dir);
        match root {
            Some(r) if d.is_relative() => r.join(d),
            _ => d,
        }
    }

    /// 证据文件的绝对路径
    pub fn file(&self, root: Option<&Path>, name: &str) -> PathBuf {
        self.abs_dir(root).join(name)
    }

    pub fn failed_step(&self) -> Option<&StepRecord> {
        self.steps.iter().find(|s| s.status == UiStepStatus::Failed)
    }
}

/// 截图文件的像素尺寸（只读文件头，不解码整张图）
pub fn image_size(path: &Path) -> Option<(u32, u32)> {
    image::image_dimensions(path).ok()
}

/// 截图上的标记
#[derive(Debug, Clone, Copy)]
pub struct Mark {
    pub rect: Rect,
    pub kind: MarkKind,
    /// 点击位置（画圆点）
    pub point: Option<(f64, f64)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkKind {
    /// 操作的元素（红框）
    Action,
    /// 断言 / 等待验证到的元素（绿框）
    Verified,
    /// 失败相关（橙框）
    Failure,
}

/// 截图最长边上限：超过等比缩小（手机竖屏 1080×2400 → 720×1600），控制证据体积
const MAX_SIDE: u32 = 1600;

/// 解码截图 → 画标记 → 缩放 → 编码 JPEG
pub fn annotate(shot: &Shot, marks: &[Mark]) -> Result<Vec<u8>> {
    let img = image::load_from_memory(&shot.bytes).context("截图解码失败")?;
    let mut rgb = img.to_rgb8();
    let (w, h) = rgb.dimensions();
    for m in marks {
        let color = match m.kind {
            MarkKind::Action => [239, 68, 68],
            MarkKind::Verified => [34, 197, 94],
            MarkKind::Failure => [249, 115, 22],
        };
        let s = shot.scale;
        let thick = ((w.max(h) as f64 / 400.0).round() as i64).clamp(2, 6);
        let pad = thick as f64;
        let x0 = (m.rect.x * s - pad).round() as i64;
        let y0 = (m.rect.y * s - pad).round() as i64;
        let x1 = ((m.rect.x + m.rect.w) * s + pad).round() as i64;
        let y1 = ((m.rect.y + m.rect.h) * s + pad).round() as i64;
        draw_rect(&mut rgb, x0, y0, x1, y1, thick, color);
        if let Some((px, py)) = m.point {
            // 空心圆环 + 中心小点：标出点击位置，又不挡住按钮上的字
            let r = (thick * 4) as f64;
            let t = thick as f64;
            draw_ring(&mut rgb, px * s, py * s, r + t, r + t * 2.0, [255, 255, 255]);
            draw_ring(&mut rgb, px * s, py * s, r, r + t, color);
            draw_ring(&mut rgb, px * s, py * s, 0.0, t, color);
        }
    }
    let out = if w.max(h) > MAX_SIDE {
        let k = MAX_SIDE as f64 / w.max(h) as f64;
        let (nw, nh) = (((w as f64) * k).round().max(1.0) as u32, ((h as f64) * k).round().max(1.0) as u32);
        image::imageops::resize(&rgb, nw, nh, image::imageops::FilterType::Triangle)
    } else {
        rgb
    };
    let mut buf = Vec::with_capacity(256 * 1024);
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 78);
    out.write_with_encoder(enc).context("截图编码失败")?;
    Ok(buf)
}

fn put(img: &mut image::RgbImage, x: i64, y: i64, c: [u8; 3]) {
    if x >= 0 && y >= 0 && (x as u32) < img.width() && (y as u32) < img.height() {
        img.put_pixel(x as u32, y as u32, image::Rgb(c));
    }
}

fn draw_rect(img: &mut image::RgbImage, x0: i64, y0: i64, x1: i64, y1: i64, thick: i64, c: [u8; 3]) {
    let (w, h) = (img.width() as i64, img.height() as i64);
    let (x0, y0, x1, y1) = (x0.clamp(0, w - 1), y0.clamp(0, h - 1), x1.clamp(0, w - 1), y1.clamp(0, h - 1));
    for t in 0..thick {
        for x in x0..=x1 {
            put(img, x, y0 + t, c);
            put(img, x, y1 - t, c);
        }
        for y in y0..=y1 {
            put(img, x0 + t, y, c);
            put(img, x1 - t, y, c);
        }
    }
}

/// 圆环（内径 r0 到外径 r1；r0 = 0 为实心圆）
fn draw_ring(img: &mut image::RgbImage, cx: f64, cy: f64, r0: f64, r1: f64, c: [u8; 3]) {
    let (x0, x1, y0, y1) = ((cx - r1).floor() as i64, (cx + r1).ceil() as i64, (cy - r1).floor() as i64, (cy + r1).ceil() as i64);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (dx, dy) = (x as f64 - cx, y as f64 - cy);
            let d2 = dx * dx + dy * dy;
            if d2 <= r1 * r1 && d2 >= r0 * r0 {
                put(img, x, y, c);
            }
        }
    }
}

/// 文字匹配：缺省忽略大小写、合并空白后包含；exact 时完全相等
pub fn text_matches(actual: &str, want: &str, exact: bool) -> bool {
    let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let (a, w) = (norm(actual), norm(want));
    if exact {
        a == w
    } else {
        a.to_lowercase().contains(&w.to_lowercase())
    }
}

/// 证据目录名：用例文件 + 名称，去掉路径分隔符等不安全字符
pub fn case_dir_name(rel: &str, name: &str) -> String {
    let stem = rel.trim_end_matches(".json").trim_start_matches("collections/");
    let base = if name.is_empty() || stem.ends_with(name) { stem.to_string() } else { format!("{stem}-{name}") };
    let mut out: String = base
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' })
        .collect();
    out = out.trim_matches(['_', '.']).to_string();
    if out.chars().count() > 80 {
        out = out.chars().take(80).collect();
    }
    if out.is_empty() {
        "case".into()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotate_draws_and_downscales() {
        let mut png = Vec::new();
        image::RgbImage::from_pixel(2000, 1000, image::Rgb([255, 255, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let shot = Shot { bytes: png, scale: 2.0 };
        let mark = Mark { rect: Rect { x: 100.0, y: 100.0, w: 50.0, h: 20.0 }, kind: MarkKind::Action, point: Some((125.0, 110.0)) };
        let jpg = annotate(&shot, &[mark]).unwrap();
        let img = image::load_from_memory(&jpg).unwrap().to_rgb8();
        assert_eq!(img.dimensions(), (1600, 800));
        // 点击点（250, 220）缩放后在 (200, 176)：中心红点
        let p = img.get_pixel(200, 176);
        assert!(p[0] > 200 && p[1] < 120, "{p:?}");
        // 红框左上角（缩放后约 (195, 155)）
        let p = img.get_pixel(158, 158);
        assert!(p[0] > 200 && p[1] < 120, "{p:?}");
    }

    #[test]
    fn text_matching_and_dir_names() {
        assert!(text_matches("  欢迎回来，\n 张三 ", "欢迎回来， 张三", false));
        assert!(text_matches("Sign In", "sign in", false));
        assert!(!text_matches("Sign In now", "Sign In", true));
        assert_eq!(case_dir_name("collections/web/01-login.json", "登录"), "web_01-login-登录");
        assert_eq!(case_dir_name("collections/web/登录.json", "登录"), "web_登录");
    }
}
