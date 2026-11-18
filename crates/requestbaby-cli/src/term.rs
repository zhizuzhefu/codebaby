use requestbaby_core::report::{CaseRef, Comparison, StepStatus};
use requestbaby_core::runner::CaseResult;
use requestbaby_core::verify;

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
        let status = r.status_text();
        let status_colored = if r.status_is_bad() {
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
        if verbose {
            for l in &r.logs {
                println!("    {DIM}[log] {l}{RESET}");
            }
        }
        if let Some(e) = &r.error {
            println!("    {RED}↳ {e}{RESET}");
        }
        let evidence = r.response_snapshot.as_ref().and_then(|s| s.evidence.as_ref());
        if let (Some(ev), true) = (evidence, verbose) {
            print_ui_steps(ev, "    ");
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
        if let Some(ev) = evidence.filter(|_| !r.passed()) {
            if let Some(shot) = ev.failed_step().and_then(|s| s.screenshot.as_ref()) {
                println!("    {DIM}失败截图 {}/{shot}{RESET}", ev.dir);
            }
            if !ev.artifacts.is_empty() {
                println!("    {DIM}失败现场 {}{RESET}", ev.artifacts.iter().map(|a| format!("{}（{}）", a.file, a.label)).collect::<Vec<_>>().join("  "));
            }
        }
    }
}

/// 界面用例的步骤时间线：✔ #1 点击 按钮「登录」  120ms  01-click.jpg
pub fn print_ui_steps(ev: &requestbaby_core::engine::ui::UiEvidence, indent: &str) {
    use requestbaby_core::engine::ui::UiStepStatus;
    for s in &ev.steps {
        let (mark, color) = match s.status {
            UiStepStatus::Passed => ("✔", GREEN),
            UiStepStatus::Failed => ("✘", RED),
            UiStepStatus::Skipped => ("⇣", YELLOW),
        };
        let shot = s.screenshot.as_deref().map(|f| format!("  {DIM}{f}{RESET}")).unwrap_or_default();
        println!("{indent}{color}{mark}{RESET} {DIM}#{}{RESET} {}  {DIM}{}ms{RESET}{shot}", s.index, s.title, s.duration_ms);
        if let Some(d) = &s.detail {
            let c = if s.status == UiStepStatus::Failed { RED } else { DIM };
            println!("{indent}    {c}↳ {}{RESET}", d.replace('\n', " "));
        }
    }
}

pub fn print_summary(results: &[CaseResult], compare: Option<&Comparison>) {
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
    if let Some(c) = compare.filter(|c| !c.is_empty()) {
        print_compare(c);
    }
    // 界面用例的截图证据：本次运行的目录
    let dirs: Vec<String> = results
        .iter()
        .filter_map(|r| r.response_snapshot.as_ref()?.evidence.as_ref())
        .map(|e| e.dir.rsplit_once('/').map(|(p, _)| p.to_string()).unwrap_or_else(|| e.dir.clone()))
        .fold(vec![], |mut acc, d| {
            if !acc.contains(&d) {
                acc.push(d);
            }
            acc
        });
    if !dirs.is_empty() {
        let shots: usize = results.iter().filter_map(|r| r.response_snapshot.as_ref()?.evidence.as_ref()).map(|e| e.steps.iter().filter(|s| s.screenshot.is_some()).count()).sum();
        println!("{DIM}截图证据 {shots} 张：{}（-v 列出每一步；--report html 生成带截图的报告）{RESET}", dirs.join("  "));
    }
    if failed > 0 {
        println!("{DIM}只重跑失败的（含其依赖的前置用例）：requestbaby run --failed{RESET}");
    }
}

/// 与历史对比：新失败 / 已修复 / 仍失败（每类最多列 8 个）
fn print_compare(c: &Comparison) {
    let names = |v: &[CaseRef]| {
        let mut s: Vec<String> = v.iter().take(8).map(|r| r.name.clone()).collect();
        if v.len() > 8 {
            s.push(format!("…共 {} 个", v.len()));
        }
        s.join(" · ")
    };
    println!(
        "对比上次：{RED}新失败 {}{RESET}  {GREEN}已修复 {}{RESET}  {YELLOW}仍失败 {}{RESET}",
        c.new_failures.len(),
        c.fixed.len(),
        c.still_failing.len()
    );
    for (label, color, v) in [("新失败", RED, &c.new_failures), ("已修复", GREEN, &c.fixed), ("仍失败", YELLOW, &c.still_failing), ("不稳定", YELLOW, &c.flaky)] {
        if !v.is_empty() {
            println!("  {color}{label}{RESET} {}", names(v));
        }
    }
}


/// 运行期提示（teardown 失败等）
pub fn print_notices(notices: &[String]) {
    for n in notices {
        eprintln!("{YELLOW}{n}{RESET}");
    }
}

/// verify 阶段渲染：开始打印阶段标题，结束打印该阶段结果
pub fn print_verify_stage(phase: verify::StagePhase<'_>) {
    match phase {
        verify::StagePhase::Started(s) => {
            if s.index == 0 {
                println!("{BOLD}verify{RESET} {DIM}stages={}{RESET}", s.total);
            }
            let planned = if s.planned > 0 { format!(" ({})", s.planned) } else { String::new() };
            println!("{DIM}── {}/{}: {}{planned}{RESET}", s.index + 1, s.total, s.label);
        }
        verify::StagePhase::Finished(_, outcome) => match outcome {
            verify::StageOutcome::Validated(issues) => {
                for i in issues.iter() {
                    let (color, sym) = if i.level == "error" { (RED, "✘") } else { (YELLOW, "⚠") };
                    println!("{color}{sym}{RESET} {} [{}]: {}", i.file, i.rule, i.message);
                }
                let errors = issues.iter().filter(|i| i.level == "error").count();
                if errors == 0 {
                    let warns = issues.len();
                    let extra = if warns > 0 { format!(" {DIM}（{warns} 个警告）{RESET}") } else { String::new() };
                    println!("{GREEN}✔{RESET} 配置校验通过{extra}");
                }
            }
            verify::StageOutcome::Skipped(why) => println!("{DIM}⇣ {why}{RESET}"),
            verify::StageOutcome::Ran(results) => print_results(results, false),
            verify::StageOutcome::Error(why) => println!("{RED}✘ {why}{RESET}"),
        },
    }
}

/// verify 结论：汇总全部阶段的用例，给出唯一结论
pub fn print_verify_verdict(out: &verify::VerifyOutcome) {
    if !out.results.is_empty() {
        print_summary(&out.results, out.report.compare.as_ref());
    } else {
        println!("{DIM}──────────────────────────────────────────{RESET}");
    }
    let stages = out
        .report
        .steps
        .iter()
        .map(|s| {
            let mark = match s.status {
                StepStatus::Passed => format!("{GREEN}✔{RESET}"),
                StepStatus::Failed => format!("{RED}✘{RESET}"),
                StepStatus::NotRun => format!("{DIM}–{RESET}"),
            };
            format!("{mark} {}", s.label)
        })
        .collect::<Vec<_>>()
        .join(&format!("{DIM}  →  {RESET}"));
    println!("{stages}");
    match (&out.blocked, out.report.failed) {
        // 详情（含服务日志）已在阶段输出中展示，结论只给一行
        (Some(why), _) => println!("{RED}{BOLD}✘ 无法完成验证{RESET}{RED}：{}{RESET}", why.lines().next().unwrap_or_default()),
        (None, 0) => println!("{GREEN}{BOLD}✔ 验证通过{RESET} {DIM}env={} · 报告 requestbaby history{RESET}", out.env),
        (None, n) => println!("{RED}{BOLD}✘ 验证未通过：{n} 个用例失败{RESET} {DIM}（--report json 查看断言详情与响应快照）{RESET}"),
    }
}
