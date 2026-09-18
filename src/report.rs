use crate::runner::CaseResult;
use serde::Serialize;

const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";

/// 单条结果流式输出
pub fn print_case(r: &CaseResult, verbose: bool) {
    let name = pad(&r.name, 28);
    let method = pad(&r.method, 7);
    let url = pad(&truncate(&r.url, 42), 42);
    let status = if r.status == 0 { "-".into() } else { r.status.to_string() };
    let status_colored = if r.status >= 400 || r.status == 0 {
        format!("{RED}{status}{RESET}")
    } else {
        format!("{GREEN}{status}{RESET}")
    };

    if r.skipped {
        println!("{YELLOW}⇣{RESET} {DIM}{name}{RESET} {method} {url} {YELLOW}skipped{RESET}");
        return;
    }

    let (mark, color) = if r.passed() {
        (format!("{GREEN}✔{RESET}"), String::new())
    } else {
        (format!("{RED}✘{RESET}"), RED.to_string())
    };
    println!(
        "{mark} {color}{name}{RESET} {DIM}{method}{RESET} {url} {status_colored} {DIM}{:>5}ms{RESET}",
        r.duration_ms.min(99999)
    );

    if verbose {
        if let Some(cmd) = &r.curl_cmd {
            println!("    {CYAN}$ {cmd}{RESET}");
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

fn pad(s: &str, w: usize) -> String {
    let len = s.chars().count();
    if len >= w {
        truncate(s, w)
    } else {
        format!("{s}{}", " ".repeat(w - len))
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    s.chars().take(n).collect::<String>() + "…"
}
