//! 屏幕文字识别（OCR）：对界面截图识别文字与位置，让没有无障碍结构的界面（微信小程序、游戏、自绘控件、
//! 远程桌面…）也能按「看到的文字」定位、断言、取值。
//!
//! 和驱动无关：作用在截图上，Web / Android / Mac 桌面三端共用。识别在运行 requestbaby 的这台机器上完成，
//! 不调用大模型、不联网、不消耗 token；同一张图每次识别结果一致。
//!
//! 识别引擎是 trait：macOS 用系统自带的 Vision 框架（进程内调用，不依赖任何外部程序或模型文件），
//! 其他平台暂未提供，会给出明确的提示。
use super::ui::Rect;
pub use crate::config::ui::Region;
use anyhow::{Context, Result};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::io::Cursor;
use std::sync::{Arc, Mutex};

/// 识别出的一行文字
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub text: String,
    /// 在整张截图上的位置（图片像素，左上角为原点）
    pub rect: Rect,
    /// 识别置信度 0.0 ~ 1.0
    pub confidence: f32,
    /// 每个字符（按 `text.chars()` 的顺序）的位置，用于只框住匹配到的那几个字；识别引擎给不出时为空
    pub glyphs: Vec<Rect>,
}

/// 一次识别的结果
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Frame {
    /// 整张截图的像素尺寸
    pub width: u32,
    pub height: u32,
    /// 按阅读顺序（自上而下，同一行自左而右）
    pub lines: Vec<Line>,
}

impl Frame {
    /// 失败现场 / `snapshot --ocr` 用的 JSON：每行的文字、位置（截图像素）、置信度
    pub fn to_json(&self) -> serde_json::Value {
        let round = |v: f64| (v * 10.0).round() / 10.0;
        serde_json::json!({
            "width": self.width,
            "height": self.height,
            "lines": self.lines.iter().map(|l| serde_json::json!({
                "text": l.text,
                "confidence": (l.confidence as f64 * 100.0).round() / 100.0,
                "rect": { "x": round(l.rect.x), "y": round(l.rect.y), "w": round(l.rect.w), "h": round(l.rect.h) },
            })).collect::<Vec<_>>(),
        })
    }
}

/// 缺省识别语言：简体中文 + 英文
pub const DEFAULT_LANGUAGES: [&str; 2] = ["zh-Hans", "en-US"];

/// 平台识别引擎：输入一张图（PNG / JPEG），输出整图坐标下的文字行
pub trait Engine: Sync {
    fn name(&self) -> &'static str;
    /// `region` 是要识别的像素范围（None = 整张图）；返回的坐标是整张图上的像素
    fn recognize(&self, image: &[u8], size: (u32, u32), region: Option<(u32, u32, u32, u32)>, languages: &[String]) -> Result<Vec<Line>>;
    /// 引擎支持的识别语言（`doctor` 用）
    fn languages(&self) -> Result<Vec<String>>;
}

#[cfg(target_os = "macos")]
fn platform_engine() -> Result<&'static dyn Engine> {
    static E: super::ocr_vision::Vision = super::ocr_vision::Vision;
    Ok(&E)
}

#[cfg(not(target_os = "macos"))]
fn platform_engine() -> Result<&'static dyn Engine> {
    anyhow::bail!("本平台暂不支持文字识别（OCR）：目前只有 macOS 提供（使用系统自带的 Vision 框架）")
}

/// 当前平台的识别引擎
pub fn engine() -> Result<&'static dyn Engine> {
    platform_engine()
}

// ---------------------------------------------------------------------------------------------
// 缓存：画面没变就不重复识别（等待元素出现时每 250ms 轮询一次，画面大多不变）
// ---------------------------------------------------------------------------------------------

const CACHE_SIZE: usize = 4;

fn cache() -> &'static Mutex<VecDeque<(u64, Arc<Frame>)>> {
    static C: Mutex<VecDeque<(u64, Arc<Frame>)>> = Mutex::new(VecDeque::new());
    &C
}

thread_local! {
    /// 本线程最近一次识别的结果（失败现场与步骤证据用）
    static LAST: RefCell<Option<Arc<Frame>>> = const { RefCell::new(None) };
    /// 本线程（= 正在跑的这个用例）使用的识别语言；空 = 缺省
    static LANGUAGES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// 设置本线程识别用的语言（项目配置 `ocr.languages`）；空 = 缺省（简体中文 + 英文）
pub fn set_languages(langs: Vec<String>) {
    LANGUAGES.with(|l| *l.borrow_mut() = langs);
}

pub fn languages() -> Vec<String> {
    LANGUAGES.with(|l| l.borrow().clone())
}

/// 本线程最近一次识别的结果，取走即清空
pub fn take_last() -> Option<Arc<Frame>> {
    LAST.with(|l| l.borrow_mut().take())
}

fn key_of(image: &[u8], region: Option<(u32, u32, u32, u32)>, languages: &[String]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    image.hash(&mut h);
    region.hash(&mut h);
    languages.hash(&mut h);
    h.finish()
}

/// 图片的像素尺寸（只读文件头）
pub fn image_dimensions(image: &[u8]) -> Result<(u32, u32)> {
    image::ImageReader::new(Cursor::new(image)).with_guessed_format().context("无法识别截图格式")?.into_dimensions().context("读取截图尺寸失败")
}

/// 识别一张图。`region` 缩小识别范围（更快也更准）；结果按图片内容缓存。
pub fn recognize(image: &[u8], region: Option<&Region>, languages: &[String]) -> Result<Arc<Frame>> {
    let size = image_dimensions(image)?;
    let px = match region {
        Some(r) => Some(r.to_pixels(size.0, size.1).context("区域 region 是空的（x / y / w / h 取 0~1，且宽高大于 0）")?),
        None => None,
    };
    let key = key_of(image, px, languages);
    if let Some(hit) = cache().lock().ok().and_then(|c| c.iter().find(|(k, _)| *k == key).map(|(_, f)| f.clone())) {
        LAST.with(|l| *l.borrow_mut() = Some(hit.clone()));
        return Ok(hit);
    }
    let langs: Vec<String> = if languages.is_empty() { DEFAULT_LANGUAGES.iter().map(|s| s.to_string()).collect() } else { languages.to_vec() };
    let mut lines = engine()?.recognize(image, size, px, &langs)?;
    sort_reading_order(&mut lines);
    let frame = Arc::new(Frame { width: size.0, height: size.1, lines });
    if let Ok(mut c) = cache().lock() {
        c.push_back((key, frame.clone()));
        while c.len() > CACHE_SIZE {
            c.pop_front();
        }
    }
    LAST.with(|l| *l.borrow_mut() = Some(frame.clone()));
    Ok(frame)
}

/// 阅读顺序：先按行（纵向重叠超过一半算同一行）自上而下，行内自左而右
pub fn sort_reading_order(lines: &mut [Line]) {
    lines.sort_by(|a, b| a.rect.y.partial_cmp(&b.rect.y).unwrap_or(std::cmp::Ordering::Equal));
    // 把纵向重叠的归到同一行，再行内按 x 排
    let mut i = 0;
    while i < lines.len() {
        let (top, bottom) = (lines[i].rect.y, lines[i].rect.y + lines[i].rect.h);
        let mut j = i + 1;
        while j < lines.len() {
            let overlap = (lines[j].rect.y + lines[j].rect.h).min(bottom) - lines[j].rect.y.max(top);
            if overlap > 0.5 * lines[j].rect.h.min(bottom - top) {
                j += 1;
            } else {
                break;
            }
        }
        lines[i..j].sort_by(|a, b| a.rect.x.partial_cmp(&b.rect.x).unwrap_or(std::cmp::Ordering::Equal));
        i = j;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(t: &str, x: f64, y: f64, w: f64, h: f64) -> Line {
        Line { text: t.into(), rect: Rect { x, y, w, h }, confidence: 1.0, glyphs: vec![] }
    }

    #[test]
    fn reading_order_groups_rows_then_sorts_left_to_right() {
        // 同一行（y 相近）里的先后顺序被打乱，下面还有一行
        let mut v = vec![line("右", 800.0, 102.0, 50.0, 40.0), line("下", 10.0, 300.0, 50.0, 40.0), line("左", 10.0, 100.0, 50.0, 40.0)];
        sort_reading_order(&mut v);
        let t: Vec<&str> = v.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(t, ["左", "右", "下"]);
    }
}
