use crate::runner::CaseResult;
use serde::Serialize;
use std::fmt::Write as _;

const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";

/// URL 展示形态：完整显示；整体超长时仅剥 query（URI path 永不截断）
fn display_url(url: &str) -> String {
    const URL_WIDTH_LIMIT: usize = 88;
    if display_width(url) <= URL_WIDTH_LIMIT {
        return url.to_string();
    }
    match url.find('?') {
        Some(i) => format!("{}?…", &url[..i]),
        None => url.to_string(),
    }
}

/// East-Asian 字符按 2 列宽计算
fn is_wide(c: char) -> bool {
    let u = c as u32;
    (0x1100..=0x115F).contains(&u)
        || (0x2E80..=0xA4CF).contains(&u)
        || (0xAC00..=0xD7A3).contains(&u)
        || (0xF900..=0xFAFF).contains(&u)
        || (0xFE30..=0xFE4F).contains(&u)
        || (0xFF00..=0xFF60).contains(&u)
        || (0xFFE0..=0xFFE6).contains(&u)
        || (0x20000..=0x3FFFD).contains(&u)
}

fn display_width(s: &str) -> usize {
    s.chars().map(|c| if is_wide(c) { 2 } else { 1 }).sum()
}

fn pad_dw(s: &str, w: usize) -> String {
    let cur = display_width(s);
    if cur >= w {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat(w - cur))
    }
}

/// 批量渲染结果表（收集后统一打印，保证列对齐）：
/// mark | 用例名(动态宽) | METHOD | URL(path 完整,超长剥 query) | 状态 | 耗时
pub fn print_results(results: &[CaseResult], verbose: bool) {
    let name_w = results.iter().map(|r| display_width(&r.name)).max().unwrap_or(0).max(4);
    let urls: Vec<String> = results.iter().map(|r| display_url(&r.url)).collect();
    let url_w = urls.iter().map(|u| display_width(u)).max().unwrap_or(3);

    for (r, url) in results.iter().zip(urls.iter()) {
        let name = pad_dw(&r.name, name_w);
        let method = pad_dw(&r.method, 7);
        let url_disp = pad_dw(url, url_w);
        let status = if r.status == 0 { "-".into() } else { r.status.to_string() };
        let status_colored = if r.status >= 400 || r.status == 0 {
            format!("{RED}{status}{RESET}")
        } else {
            format!("{GREEN}{status}{RESET}")
        };

        if r.skipped {
            println!("{YELLOW}⇣{RESET} {name} {DIM}{method}{RESET} {url_disp} {YELLOW}skipped{RESET}");
            continue;
        }

        let (mark, color) = if r.passed() {
            (format!("{GREEN}✔{RESET}"), String::new())
        } else {
            (format!("{RED}✘{RESET}"), RED.to_string())
        };
        println!(
            "{mark} {color}{name}{RESET} {DIM}{method}{RESET} {url_disp} {status_colored} {DIM}{:>6}ms{RESET}",
            r.duration_ms.min(999999)
        );

        if verbose {
            if let Some(cmd) = &r.curl_cmd {
                println!("    {CYAN}$ {cmd}{RESET}");
            }
            if let Some(b) = &r.body_preview {
                println!("    {DIM}body: {b}{RESET}");
            }
        }
        if let Some(e) = &r.error {
            println!("    {RED}↳ {e}{RESET}");
        }
        for (i, t) in r.tests.iter().enumerate() {
            if t.pass && !verbose {
                continue;
            }
            let (sym, c) = if t.pass {
                (GREEN, String::new())
            } else {
                (RED, RED.to_string())
            };
            println!("    {sym}{sym}{RESET} {c}[{i}] {}{RESET}", t.expr);
            if let Some(d) = &t.detail {
                let d = d.replace('\n', " ");
                println!("      {DIM}↳ {d}{RESET}");
            }
        }
    }
}

pub fn print_summary(results: &[CaseResult]) {
    let total = results.len();
    let passed = results.iter().filter(|r| r.passed()).count();
    let skipped = results.iter().filter(|r| r.skipped).count();
    let failed = total - passed - skipped;
    let dur: u64 = results.iter().map(|r| r.duration_ms).sum();
    println!("{DIM}──────────────────────────────────────────{RESET}");
    let tail = if failed > 0 {
        format!("{RED}✘ {failed} failed{RESET}  ")
    } else {
        String::new()
    };
    println!(
        "{BOLD}{total}{RESET} cases: {GREEN}✔ {passed} passed{RESET}  {tail}{YELLOW}⇣ {skipped} skipped{RESET}  {DIM}| {dur}ms{RESET}"
    );
}

#[derive(Serialize)]
struct JsonTest {
    expr: String,
    pass: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

#[derive(Serialize)]
struct JsonResult {
    file: String,
    name: String,
    tags: Vec<String>,
    method: String,
    url: String,
    status: u16,
    duration_ms: u64,
    skipped: bool,
    passed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    curl: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response: Option<crate::runner::ResponseSnapshot>,
    tests: Vec<JsonTest>,
}

#[derive(Serialize)]
struct JsonReport {
    total: usize,
    passed: usize,
    failed: usize,
    skipped: usize,
    results: Vec<JsonResult>,
}

/// JUnit XML（CI 平台渲染）
pub fn render_junit(results: &[CaseResult], total_ms: u64) -> String {
    let total = results.len();
    let failures = results.iter().filter(|r| !r.passed() && !r.skipped).count();
    let skipped = results.iter().filter(|r| r.skipped).count();
    let mut x = String::new();
    let _ = write!(
        x,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites tests=\"{total}\" failures=\"{failures}\" skipped=\"{skipped}\" time=\"{:.3}\">\n",
        total_ms as f64 / 1000.0
    );
    for r in results {
        let _ = write!(
            x,
            "  <testcase classname=\"{}\" name=\"{}\" time=\"{:.3}\">",
            xml_escape(&r.rel),
            xml_escape(&r.name),
            r.duration_ms as f64 / 1000.0
        );
        if r.skipped {
            let _ = write!(x, "<skipped/>");
        } else if let Some(e) = &r.error {
            let _ = write!(x, "<failure message=\"{}\">{}</failure>", xml_escape(e), xml_escape(e));
        } else if let Some(t) = r.tests.iter().find(|t| !t.pass) {
            let _ = write!(
                x,
                "<failure message=\"{}\">{}</failure>",
                xml_escape(&t.expr),
                xml_escape(t.detail.as_deref().unwrap_or(""))
            );
        }
        let _ = write!(x, "</testcase>\n");
    }
    let _ = write!(x, "</testsuites>\n");
    x
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// 单文件 HTML（内嵌数据 + 极简渲染）
pub fn render_html(results: &[CaseResult]) -> String {
    let data = render_json(results, false);
    format!(
        r#"<!DOCTYPE html><html lang="zh"><head><meta charset="utf-8"><title>requestbaby report</title>
<style>
body{{font-family:-apple-system,'PingFang SC',monospace;margin:24px;background:#0f172a;color:#e2e8f0}}
table{{border-collapse:collapse;width:100%}}
td,th{{padding:6px 10px;border-bottom:1px solid #1e293b;text-align:left;font-size:13px}}
.pass{{color:#4ade80}}.fail{{color:#f87171}}.skip{{color:#fbbf24}}.dim{{color:#64748b}}
details{{margin:4px 0}}pre{{background:#1e293b;padding:8px;border-radius:6px;overflow:auto;font-size:12px}}
h1{{font-size:18px}}
</style></head><body>
<h1>requestbaby 报告</h1>
<table id="t"></table>
<script>
const R = {data};
const t = document.getElementById('t');
t.innerHTML = '<tr><th>结果</th><th>用例</th><th>方法</th><th>URL</th><th>状态</th><th>耗时</th></tr>' +
R.results.map(r => {{
const cls = r.skipped ? 'skip' : (r.passed ? 'pass' : 'fail');
const mark = r.skipped ? '⇣' : (r.passed ? '✔' : '✘');
const detail = (r.error ? `<div class="fail">${{esc(r.error)}}</div>` : '') +
(r.tests||[]).filter(x=>!x.pass).map(x=>`<div>✘ ${{esc(x.expr)}}<pre>${{esc(x.detail||'')}}</pre></div>`).join('');
return `<tr><td class="${{cls}}">${{mark}}</td><td>${{esc(r.name)}}${{detail?`<details><summary class="dim">详情</summary>${{detail}}</details>`:''}}</td><td class="dim">${{r.method}}</td><td>${{esc(r.url)}}</td><td class="${{r.status>=400?'fail':'pass'}}">${{r.status||'-'}}</td><td class="dim">${{r.duration_ms}}ms</td></tr>`;
}}).join('');
function esc(s){{return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;')}}
</script></body></html>"#
    )
}

pub fn render_json(results: &[CaseResult], include_curl: bool) -> String {
    let report = JsonReport {
        total: results.len(),
        passed: results.iter().filter(|r| r.passed()).count(),
        failed: results.iter().filter(|r| !r.passed() && !r.skipped).count(),
        skipped: results.iter().filter(|r| r.skipped).count(),
        results: results
            .iter()
            .map(|r| JsonResult {
                file: r.rel.clone(),
                name: r.name.clone(),
                tags: r.tags.clone(),
                method: r.method.clone(),
                url: r.url.clone(),
                status: r.status,
                duration_ms: r.duration_ms,
                skipped: r.skipped,
                passed: r.passed(),
                error: r.error.clone(),
                curl: if include_curl { r.curl_cmd.clone() } else { None },
                response: r.response_snapshot.clone(),
                tests: r
                    .tests
                    .iter()
                    .map(|t| JsonTest { expr: t.expr.clone(), pass: t.pass, detail: t.detail.clone() })
                    .collect(),
            })
            .collect(),
    };
    serde_json::to_string_pretty(&report).unwrap_or_default()
}
