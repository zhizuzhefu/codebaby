//! macOS 文字识别：系统自带的 Vision 框架（VNRecognizeTextRequest），进程内调用。
//!
//! - 不 spawn 外部进程、不带模型文件；识别在本机完成（神经网络引擎加速）
//! - 中文识别需要 macOS 13 及以上；更低版本只能识别英文等少数语言，`doctor` 会提示
use super::ocr::{Engine, Line};
use super::ui::Rect;
use anyhow::{anyhow, bail, Result};
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::AnyObject;
use objc2::AnyThread;
use objc2_foundation::{NSArray, NSData, NSDictionary, NSRange, NSString};
use objc2_vision::{VNImageOption, VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel};

pub struct Vision;

impl Engine for Vision {
    fn name(&self) -> &'static str {
        "macOS Vision"
    }

    fn recognize(&self, image: &[u8], size: (u32, u32), region: Option<(u32, u32, u32, u32)>, languages: &[String]) -> Result<Vec<Line>> {
        autoreleasepool(|_| {
            let (w, h) = (size.0 as f64, size.1 as f64);
            // Vision 返回的归一化坐标相对于「识别区域」（没有区域就是整图），不是相对整图
            let area = match region {
                Some((x, y, rw, rh)) => (x as f64, y as f64, rw as f64, rh as f64),
                None => (0.0, 0.0, w, h),
            };
            let data = NSData::with_bytes(image);
            let options = NSDictionary::<VNImageOption, AnyObject>::new();
            let handler = VNImageRequestHandler::initWithData_options(VNImageRequestHandler::alloc(), &data, &options);

            let request = VNRecognizeTextRequest::new();
            request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
            // 关掉语言纠错：纠错会把「已约满」这类专有词「纠正」成常见词，要的是屏幕上原样的字
            request.setUsesLanguageCorrection(false);
            let langs: Vec<Retained<NSString>> = languages.iter().map(|l| NSString::from_str(l)).collect();
            request.setRecognitionLanguages(&NSArray::from_retained_slice(&langs));
            if let Some((x, y, rw, rh)) = region {
                // 归一化坐标，原点在左下角
                let roi = objc2_core_foundation::CGRect::new(
                    objc2_core_foundation::CGPoint::new(x as f64 / w, 1.0 - (y + rh) as f64 / h),
                    objc2_core_foundation::CGSize::new(rw as f64 / w, rh as f64 / h),
                );
                unsafe { request.setRegionOfInterest(roi) };
            }
            let as_request: &VNRequest = &request;
            handler.performRequests_error(&NSArray::from_slice(&[as_request])).map_err(|e| anyhow!("Vision 识别失败：{}", e.localizedDescription()))?;

            let mut out = vec![];
            let Some(results) = request.results() else { return Ok(out) };
            for obs in results.iter() {
                let Some(top) = obs.topCandidates(1).firstObject() else { continue };
                let text = top.string().to_string();
                if text.trim().is_empty() {
                    continue;
                }
                let confidence = top.confidence();
                let bb = unsafe { obs.boundingBox() };
                let rect = to_pixels(bb.origin.x, bb.origin.y, bb.size.width, bb.size.height, area);
                // 每个字符的位置：按 UTF-16 偏移向 Vision 要子串的框
                let mut glyphs = Vec::with_capacity(text.chars().count());
                let mut off = 0usize;
                for c in text.chars() {
                    let n = c.len_utf16();
                    let r = unsafe { top.boundingBoxForRange_error(NSRange::new(off, n)) }.ok().map(|o| {
                        let b = unsafe { o.boundingBox() };
                        to_pixels(b.origin.x, b.origin.y, b.size.width, b.size.height, area)
                    });
                    match r {
                        Some(r) => glyphs.push(r),
                        None => {
                            glyphs.clear();
                            break;
                        }
                    }
                    off += n;
                }
                out.push(Line { text, rect, confidence, glyphs });
            }
            Ok(out)
        })
    }

    fn languages(&self) -> Result<Vec<String>> {
        autoreleasepool(|_| {
            let request = VNRecognizeTextRequest::new();
            request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
            let langs = unsafe { request.supportedRecognitionLanguagesAndReturnError() }.map_err(|e| anyhow!("读取 Vision 支持的语言失败：{}", e.localizedDescription()))?;
            let out: Vec<String> = langs.iter().map(|s| s.to_string()).collect();
            if out.is_empty() {
                bail!("Vision 没有返回任何支持的语言");
            }
            Ok(out)
        })
    }
}

/// Vision 的归一化框（原点在左下角，相对识别区域）→ 整图像素（原点在左上角）；area = 识别区域的像素 (x, y, 宽, 高)
fn to_pixels(x: f64, y: f64, w: f64, h: f64, area: (f64, f64, f64, f64)) -> Rect {
    let (ax, ay, aw, ah) = area;
    Rect { x: ax + x * aw, y: ay + (1.0 - y - h) * ah, w: w * aw, h: h * ah }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_box_flips_vertical_axis() {
        // 左下角原点：y=0.9、高 0.05 的框在图的最上方
        let r = to_pixels(0.1, 0.9, 0.5, 0.05, (0.0, 0.0, 1000.0, 2000.0));
        assert_eq!((r.x, r.w, r.h), (100.0, 500.0, 100.0));
        assert!((r.y - 100.0).abs() < 1e-9);
        // 识别区域不在原点时，结果要平移回整图坐标：区域 (200,300) 大小 400x600，框占区域左上角 1/10
        let r = to_pixels(0.0, 0.9, 0.1, 0.1, (200.0, 300.0, 400.0, 600.0));
        assert!((r.x - 200.0).abs() < 1e-9 && (r.y - 300.0).abs() < 1e-9 && (r.w - 40.0).abs() < 1e-9 && (r.h - 60.0).abs() < 1e-9);
    }
}
