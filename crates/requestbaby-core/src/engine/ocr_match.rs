//! 屏幕文字识别的匹配：把识别出的文字行按定位（`ocr` / `region` / `row` / `exact` / `fuzzy` / `minConfidence`）
//! 找出命中的位置，换算成各驱动通用的 `Element`，交给步骤循环去等待、断言、点击、取值。
//!
//! 匹配规则（缺省完全确定，同一张图每次结论一致）：
//! - 先归一化再比：去掉所有空白（中文识别常多出或漏掉空格）、全角转半角（`￥` / `：` / `（`）、统一连字符、忽略大小写
//! - 缺省「包含」；`exact` 要求整行相等；`fuzzy: n` 允许相差 n 个字（识别把「已」认成「己」这类）
//! - 命中框只框住匹配到的那几个字，不是整行（点击点击的是它们的中心）
use super::ocr::{self, Frame, Line};
use super::ui::{Element, Rect, UiDriver};
use crate::config::Locator;
use anyhow::Result;

/// 缺省的最低置信度：低于它的识别结果忽略（状态栏图标、单字常落在 0.3 附近）
pub const DEFAULT_MIN_CONFIDENCE: f32 = 0.3;

/// 一处命中
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// 在 Frame.lines 里的序号
    pub line: usize,
    /// 命中的位置（整图像素）
    pub rect: Rect,
    /// 整行文字（取值、`hasText` 用）；`*` 时为区域内全部文字
    pub text: String,
    /// 匹配到的那几个字
    pub matched: String,
    pub confidence: f32,
}

/// 一次查找的条件
#[derive(Debug, Clone)]
pub struct Search<'a> {
    pub text: &'a str,
    pub exact: bool,
    pub fuzzy: usize,
    pub min_confidence: f32,
    pub row: Option<&'a str>,
}

impl<'a> Search<'a> {
    pub fn from_locator(loc: &'a Locator) -> Option<Search<'a>> {
        Some(Search {
            text: loc.ocr.as_deref()?,
            exact: loc.exact.unwrap_or(false),
            fuzzy: loc.fuzzy.unwrap_or(0) as usize,
            min_confidence: loc.min_confidence.map(|c| c as f32).unwrap_or(DEFAULT_MIN_CONFIDENCE),
            row: loc.row.as_deref(),
        })
    }
}

// ---------------------------------------------------------------------------------------------
// 归一化
// ---------------------------------------------------------------------------------------------

/// 一个字符归一化后的样子；None = 丢弃（空白）
fn fold(c: char) -> Option<char> {
    if c.is_whitespace() {
        return None;
    }
    Some(match c {
        // 全角 ASCII（！～）→ 半角
        '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
        '\u{FFE5}' => '¥',
        // 各种连字符 / 减号 / 破折号
        '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
        '\u{2018}' | '\u{2019}' => '\'',
        '\u{201C}' | '\u{201D}' => '"',
        _ => c,
    })
}

/// 归一化：返回归一化后的字符，以及每个字符对应原文的第几个字符（用于把命中位置映射回原文）
pub fn normalize(s: &str) -> (Vec<char>, Vec<usize>) {
    let (mut out, mut map) = (vec![], vec![]);
    for (i, c) in s.chars().enumerate() {
        let Some(c) = fold(c) else { continue };
        for l in c.to_lowercase() {
            out.push(l);
            map.push(i);
        }
    }
    (out, map)
}

fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push((prev[j] + usize::from(ca != cb)).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// 在一行文字里找出命中的字符范围：(起, 止（不含）, 编辑距离)，下标是原文的字符序号
pub fn find_spans(text: &str, want: &str, exact: bool, fuzzy: usize) -> Vec<(usize, usize, usize)> {
    let (nt, map) = normalize(text);
    let (nw, _) = normalize(want);
    if nw.is_empty() || nt.is_empty() {
        return vec![];
    }
    let total = text.chars().count();
    if exact {
        let d = levenshtein(&nt, &nw);
        return if d <= fuzzy { vec![(0, total, d)] } else { vec![] };
    }
    let m = nw.len();
    let span = |s: usize, l: usize, d: usize| (map[s], map[s + l - 1] + 1, d);
    // 先找完全相同的（不重叠）
    let mut out = vec![];
    let mut i = 0;
    while i + m <= nt.len() {
        if nt[i..i + m] == nw[..] {
            out.push(span(i, m, 0));
            i += m;
        } else {
            i += 1;
        }
    }
    if !out.is_empty() || fuzzy == 0 {
        return out;
    }
    // 没有完全相同的：允许相差 fuzzy 个字，自左向右取最接近的窗口
    let mut i = 0;
    while i < nt.len() {
        let mut best: Option<(usize, usize)> = None; // (距离, 窗口长度)
        for l in m.saturating_sub(fuzzy).max(1)..=(m + fuzzy).min(nt.len() - i) {
            let d = levenshtein(&nw, &nt[i..i + l]);
            if d <= fuzzy && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, l));
            }
        }
        match best {
            Some((d, l)) => {
                out.push(span(i, l, d));
                i += l;
            }
            None => i += 1,
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// 位置
// ---------------------------------------------------------------------------------------------

fn union(rects: impl Iterator<Item = Rect>) -> Option<Rect> {
    rects.reduce(|a, b| {
        let (x0, y0) = (a.x.min(b.x), a.y.min(b.y));
        let (x1, y1) = ((a.x + a.w).max(b.x + b.w), (a.y + a.h).max(b.y + b.h));
        Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
    })
}

/// 一行文字里第 start..end 个字符的位置：有逐字位置就合并，没有就按字宽（汉字 1、其他 0.55）估算
pub fn span_rect(line: &Line, start: usize, end: usize) -> Rect {
    let n = line.text.chars().count();
    if line.glyphs.len() == n {
        if let Some(r) = union(line.glyphs[start..end].iter().copied()) {
            return r;
        }
    }
    let weights: Vec<f64> = line.text.chars().map(|c| if c.is_ascii() { 0.55 } else { 1.0 }).collect();
    let total: f64 = weights.iter().sum::<f64>().max(1e-9);
    let before: f64 = weights[..start].iter().sum();
    let inside: f64 = weights[start..end].iter().sum();
    Rect { x: line.rect.x + line.rect.w * before / total, y: line.rect.y, w: line.rect.w * inside / total, h: line.rect.h }
}

/// 同一横排的纵向范围：行锚点上下各放宽 30% 行高（卡片里文字的基线并不严格对齐）
fn band(r: &Rect) -> (f64, f64) {
    (r.y - 0.3 * r.h, r.y + r.h * 1.3)
}

fn same_row(a: &Rect, b: &Rect) -> bool {
    let overlap = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
    overlap > 0.5 * a.h.min(b.h)
}

// ---------------------------------------------------------------------------------------------
// 查找
// ---------------------------------------------------------------------------------------------

/// 在识别结果里找：按阅读顺序返回全部命中
pub fn search(frame: &Frame, q: &Search) -> Vec<Hit> {
    let lines: Vec<(usize, &Line)> = frame.lines.iter().enumerate().filter(|(_, l)| l.confidence >= q.min_confidence).collect();
    // 同一横排：先找行锚点，命中必须落在某个锚点所在的横排里
    let bands: Option<Vec<(f64, f64)>> = q.row.map(|r| {
        lines
            .iter()
            .filter(|(_, l)| !find_spans(&l.text, r, false, q.fuzzy).is_empty())
            .map(|(_, l)| band(&l.rect))
            .collect()
    });
    let in_row = |r: &Rect| match &bands {
        None => true,
        Some(b) => {
            let cy = r.y + r.h / 2.0;
            b.iter().any(|(lo, hi)| cy >= *lo && cy <= *hi)
        }
    };

    if q.text.trim() == "*" {
        return all_text(&lines, &in_row).into_iter().collect();
    }
    let mut hits = vec![];
    for (i, l) in &lines {
        for (s, e, _) in find_spans(&l.text, q.text, q.exact, q.fuzzy) {
            let rect = span_rect(l, s, e);
            if in_row(&rect) {
                hits.push(Hit { line: *i, rect, text: l.text.clone(), matched: l.text.chars().skip(s).take(e - s).collect(), confidence: l.confidence });
            }
        }
    }
    hits
}

/// `ocr: "*"`：区域内的全部文字合成一处（同一横排的用空格连接，换行分行）
fn all_text(lines: &[(usize, &Line)], in_row: &dyn Fn(&Rect) -> bool) -> Option<Hit> {
    let picked: Vec<&(usize, &Line)> = lines.iter().filter(|(_, l)| in_row(&l.rect)).collect();
    let first = picked.first()?;
    let mut text = String::new();
    let mut prev: Option<&Rect> = None;
    for (_, l) in &picked {
        if let Some(p) = prev {
            text.push(if same_row(p, &l.rect) { ' ' } else { '\n' });
        }
        text.push_str(&l.text);
        prev = Some(&l.rect);
    }
    let rect = union(picked.iter().map(|(_, l)| l.rect))?;
    let confidence = picked.iter().map(|(_, l)| l.confidence).fold(1.0_f32, f32::min);
    Some(Hit { line: first.0, rect, text: text.clone(), matched: text, confidence })
}

// ---------------------------------------------------------------------------------------------
// 与驱动对接
// ---------------------------------------------------------------------------------------------

/// 把命中换算成各驱动通用的元素：坐标从截图像素换回驱动自己的坐标系（`scale` = 坐标到图片像素的倍数）
pub fn element_of(hit: &Hit, scale: f64, frame: &Frame) -> Element {
    let s = if scale > 0.0 { scale } else { 1.0 };
    let rect = Rect { x: hit.rect.x / s, y: hit.rect.y / s, w: hit.rect.w / s, h: hit.rect.h / s };
    let inside = hit.rect.x + hit.rect.w > 0.0 && hit.rect.y + hit.rect.h > 0.0 && hit.rect.x < frame.width as f64 && hit.rect.y < frame.height as f64;
    Element {
        handle: usize::MAX,
        rect,
        visible: inside && !rect.is_empty(),
        enabled: true,
        // 按文字点进输入框再填：fill 时点它、清空、输入
        editable: true,
        text: hit.text.clone(),
        value: Some(hit.text.clone()),
        tag: format!("屏幕文字「{}」", hit.matched.chars().take(30).collect::<String>().replace('\n', " ")),
        visual: true,
        confidence: Some(hit.confidence),
        ..Default::default()
    }
}

thread_local! {
    /// 本线程最近一次查找的摘要（步骤详情 / 失败原因里告诉用户屏幕上识别到了什么）
    static NOTE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    /// 本线程最近一次识别用的截图：不改变画面的步骤（断言 / 等待）直接拿它当证据，省一次截图（远程设备上截一张要半秒）
    static LAST_SHOT: std::cell::RefCell<Option<(std::time::Instant, super::ui::Shot)>> = const { std::cell::RefCell::new(None) };
}

/// 取走最近一次识别用的截图（超过 `max_age` 的不要：画面可能已经变了）
pub fn take_fresh_shot(max_age: std::time::Duration) -> Option<super::ui::Shot> {
    LAST_SHOT.with(|s| s.borrow_mut().take()).filter(|(t, _)| t.elapsed() <= max_age).map(|(_, s)| s)
}

/// 最近一次 OCR 查找的摘要，取走即清空
pub fn take_note() -> Option<String> {
    NOTE.with(|n| n.borrow_mut().take())
}

/// 摘要：命中几处与置信度；没命中就列出屏幕上识别到的文字，方便对照着改写法
fn summarize(frame: &Frame, hits: &[Hit], whole: bool) -> String {
    if frame.lines.is_empty() {
        return "没有识别到任何文字（截图是空白的？region 是不是太小？）".into();
    }
    if whole {
        return format!("识别到 {} 行文字", frame.lines.len());
    }
    if !hits.is_empty() {
        let (lo, hi) = hits.iter().fold((1.0_f32, 0.0_f32), |(lo, hi), h| (lo.min(h.confidence), hi.max(h.confidence)));
        return if (hi - lo).abs() < 0.005 { format!("OCR 命中 {} 处（置信度 {hi:.2}）", hits.len()) } else { format!("OCR 命中 {} 处（置信度 {lo:.2}～{hi:.2}）", hits.len()) };
    }
    let mut seen: Vec<&str> = vec![];
    for l in &frame.lines {
        if !seen.contains(&l.text.as_str()) {
            seen.push(&l.text);
        }
    }
    let mut shown = String::new();
    let mut count = 0;
    for t in &seen {
        if shown.chars().count() + t.chars().count() > 120 {
            break;
        }
        if !shown.is_empty() {
            shown.push('、');
        }
        shown.push_str(t);
        count += 1;
    }
    format!("屏幕上识别到 {} 行文字：{shown}{}", frame.lines.len(), if count < seen.len() { "…" } else { "" })
}

/// 截图 → 识别 → 按定位找，返回命中的元素（驱动无关：Web / Android / Mac 桌面共用）
pub fn find(d: &mut dyn UiDriver, loc: &Locator) -> Result<Vec<Element>> {
    let Some(q) = Search::from_locator(loc) else { return Ok(vec![]) };
    let shot = d.screenshot_for_ocr()?;
    let frame = ocr::recognize(&shot.bytes, loc.region.as_ref(), &ocr::languages())?;
    let hits = search(&frame, &q);
    let note = summarize(&frame, &hits, q.text.trim() == "*");
    NOTE.with(|n| *n.borrow_mut() = Some(note));
    let elements = hits.iter().map(|h| element_of(h, shot.scale, &frame)).collect();
    LAST_SHOT.with(|s| *s.borrow_mut() = Some((std::time::Instant::now(), shot)));
    Ok(elements)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { x, y, w, h }
    }

    /// 没有逐字位置的行（按字宽估算）
    fn line(text: &str, rect: Rect, confidence: f32) -> Line {
        Line { text: text.into(), rect, confidence, glyphs: vec![] }
    }

    fn frame(lines: Vec<Line>) -> Frame {
        Frame { width: 1080, height: 2400, lines }
    }

    fn q(text: &str) -> Search<'_> {
        Search { text, exact: false, fuzzy: 0, min_confidence: DEFAULT_MIN_CONFIDENCE, row: None }
    }

    #[test]
    fn normalizes_whitespace_width_and_case() {
        assert_eq!(normalize("挂号费： ￥ 80").0.iter().collect::<String>(), "挂号费:¥80");
        assert_eq!(normalize("Sign  In").0.iter().collect::<String>(), "signin");
        assert_eq!(normalize("11–24").0.iter().collect::<String>(), "11-24");
        // 映射回原文的字符序号：空白被丢弃，后面的字符序号照原文
        let (n, map) = normalize("a b");
        assert_eq!((n.len(), map), (2, vec![0, 2]));
    }

    #[test]
    fn finds_all_occurrences_and_maps_back_to_original_chars() {
        // 一行里出现两次
        assert_eq!(find_spans("已约满 已约满", "已约满", false, 0), vec![(0, 3, 0), (4, 7, 0)]);
        // 包含 / 完全相等
        assert_eq!(find_spans("王媛媛 主任医师", "主任医师", false, 0), vec![(4, 8, 0)]);
        assert!(find_spans("王媛媛 主任医师", "主任医师", true, 0).is_empty());
        assert_eq!(find_spans("已 约 满", "已约满", true, 0), vec![(0, 5, 0)]);
        assert!(find_spans("其他院区号源查看", "已约满", false, 0).is_empty());
        // 空的期望文字不匹配任何东西
        assert!(find_spans("abc", "  ", false, 0).is_empty());
    }

    #[test]
    fn fuzzy_tolerates_misrecognized_characters_only_when_asked() {
        assert!(find_spans("己约满", "已约满", false, 0).is_empty());
        assert_eq!(find_spans("11-24 己约满", "已约满", false, 1), vec![(6, 9, 1)]);
        // 有完全相同的就不再做模糊匹配
        assert_eq!(find_spans("己约满 已约满", "已约满", false, 1), vec![(4, 7, 0)]);
        // 相差太多
        assert!(find_spans("未放号", "已约满", false, 1).is_empty());
        // exact + fuzzy：整行允许相差 n 个字
        assert_eq!(find_spans("己约满", "已约满", true, 1), vec![(0, 3, 1)]);
    }

    #[test]
    fn span_rect_uses_glyph_boxes_or_estimates_by_width() {
        // 有逐字位置：合并命中那几个字
        let l = Line { text: "ab已".into(), rect: r(0.0, 10.0, 90.0, 20.0), confidence: 1.0, glyphs: vec![r(0.0, 10.0, 20.0, 20.0), r(20.0, 10.0, 20.0, 20.0), r(50.0, 11.0, 40.0, 18.0)] };
        assert_eq!(span_rect(&l, 1, 3), r(20.0, 10.0, 70.0, 20.0));
        // 没有：按字宽估算，汉字权重 1、其他 0.55
        let l = line("ab已约", r(0.0, 0.0, 310.0, 40.0), 1.0);
        let x = span_rect(&l, 2, 4);
        assert!((x.x - 110.0).abs() < 1e-6 && (x.w - 200.0).abs() < 1e-6, "{x:?}");
    }

    #[test]
    fn search_orders_hits_filters_low_confidence_and_picks_by_nth_outside() {
        let f = frame(vec![
            line("11-24", r(70.0, 330.0, 100.0, 40.0), 1.0),
            line("已约满", r(70.0, 377.0, 108.0, 42.0), 0.5),
            line("已约满", r(258.0, 377.0, 105.0, 42.0), 1.0),
            line("已约满", r(878.0, 610.0, 132.0, 49.0), 0.2),
        ]);
        let hits = search(&f, &q("已约满"));
        // 置信度 0.2 的被忽略
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].rect.x, 70.0);
        assert_eq!(hits[1].rect.x, 258.0);
        // 调低门槛就都能看到
        let low = Search { min_confidence: 0.1, ..q("已约满") };
        assert_eq!(search(&f, &low).len(), 3);
    }

    #[test]
    fn row_keeps_only_hits_on_the_anchor_row() {
        // 两张卡片：各有「姓名职称」和一个「已约满」
        let f = frame(vec![
            line("王媛媛 主任医师", r(240.0, 610.0, 327.0, 53.0), 1.0),
            line("已约满", r(878.0, 610.0, 132.0, 49.0), 1.0),
            line("挂号费：¥80", r(240.0, 705.0, 282.0, 45.0), 1.0),
            line("贾维茜 主治医师", r(240.0, 883.0, 331.0, 52.0), 1.0),
            line("预约", r(878.0, 883.0, 132.0, 45.0), 1.0),
        ]);
        let on_first = Search { row: Some("王媛媛"), ..q("已约满") };
        let hits = search(&f, &on_first);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].rect.y, 610.0);
        // 第二张卡片那一行没有「已约满」
        assert!(search(&f, &Search { row: Some("贾维茜"), ..q("已约满") }).is_empty());
        // 找不到锚点 = 没有命中，不报错（页面可能还在加载）
        assert!(search(&f, &Search { row: Some("不存在的人"), ..q("已约满") }).is_empty());
    }

    #[test]
    fn star_collects_all_text_in_reading_order() {
        let f = frame(vec![
            line("挂号费：", r(240.0, 705.0, 140.0, 45.0), 1.0),
            line("¥80", r(390.0, 706.0, 60.0, 44.0), 1.0),
            line("其他院区号源查看", r(714.0, 698.0, 300.0, 45.0), 1.0),
            line("已约满", r(878.0, 900.0, 132.0, 49.0), 1.0),
        ]);
        let hits = search(&f, &q("*"));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text, "挂号费： ¥80 其他院区号源查看\n已约满");
        assert_eq!(hits[0].rect, r(240.0, 698.0, 774.0, 251.0));
        // 加 row：只取同一横排
        let one = search(&f, &Search { row: Some("挂号费"), ..q("*") });
        assert_eq!(one[0].text, "挂号费： ¥80 其他院区号源查看");
        // 什么都没识别到
        assert!(search(&frame(vec![]), &q("*")).is_empty());
    }

    #[test]
    fn element_converts_pixels_back_to_driver_coordinates() {
        let f = frame(vec![line("已约满", r(878.0, 610.0, 132.0, 50.0), 0.9)]);
        let hit = &search(&f, &q("已约满"))[0];
        // Mac 桌面：截图是 2 倍，元素坐标是点
        let el = element_of(hit, 2.0, &f);
        assert!(el.visual && el.visible && el.enabled);
        assert_eq!(el.rect, r(439.0, 305.0, 66.0, 25.0));
        assert_eq!(el.text, "已约满");
        assert_eq!(el.confidence, Some(0.9));
        // Android：1 倍
        assert_eq!(element_of(hit, 1.0, &f).rect, r(878.0, 610.0, 132.0, 50.0));
    }
}
