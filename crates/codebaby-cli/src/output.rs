//! 报告输出与运行事件的终端 / JSON 流钩子

use super::*;

/// 统一报告输出（run / workflow 共用）
pub(crate) fn emit_report(fmt: &str, results: &[runner::CaseResult], run_report: &report::RunReport, verbose: bool) {
    match fmt {
        "json" => {
            let mut r = report::RunReport::new(results, verbose, run_report.meta.clone());
            r.steps = run_report.steps.clone();
            r.compare = run_report.compare.clone();
            println!("{}", report::render_json(&r));
        }
        "junit" => {
            let mut r = report::RunReport::new(results, verbose, run_report.meta.clone());
            r.steps = run_report.steps.clone();
            r.compare = run_report.compare.clone();
            print!("{}", report::render_junit(&r));
        }
        "html" => {
            let path = PathBuf::from("report.html");
            let root = std::env::current_dir().ok().and_then(|d| session::project_root(&d));
            codebaby_core::fsx::write(&path, report::render_html(run_report, root.as_deref())).ok();
            eprintln!("\x1b[2mHTML 报告: {}\x1b[0m", path.display());
            term::print_summary(results, run_report.compare.as_ref());
        }
        _ => {
            term::print_results(results, verbose);
            term::print_summary(results, run_report.compare.as_ref());
        }
    }
}

pub(crate) fn check_report_format(fmt: &str) -> Result<(), String> {
    match fmt {
        "term" | "json" | "html" | "junit" => Ok(()),
        other => Err(format!("不支持的报告格式 {other}（可选 term / json / html / junit）")),
    }
}

pub(crate) fn json_stream_hook() -> runner::EventHook {
    Arc::new(|ev: &runner::RunEvent| {
        use std::io::Write;
        let line = serde_json::to_string(ev).unwrap_or_default();
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    })
}

/// 运行事件回调：`--json-stream` 输出全部事件到 stdout；终端里的普通运行实时打印界面用例每一步的进度（stderr，不污染 stdout）
pub(crate) fn run_hook(json_stream: bool, term: bool) -> Option<runner::EventHook> {
    use std::io::IsTerminal;
    if json_stream {
        Some(json_stream_hook())
    } else if term && std::io::stderr().is_terminal() {
        Some(live_steps_hook())
    } else {
        None
    }
}

/// 界面用例（Web / Android）跑得慢：每完成一步就打印一行，不必等用例结束才知道做到哪了
pub(crate) fn live_steps_hook() -> runner::EventHook {
    let last = std::sync::Mutex::new(String::new());
    Arc::new(move |ev: &runner::RunEvent| {
        let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
        match ev {
            runner::RunEvent::CaseStarted { .. } => last.clear(),
            runner::RunEvent::UiStep { file, step, .. } => {
                use codebaby_core::engine::ui::UiStepStatus;
                if *last != *file {
                    eprintln!("\x1b[2m▶ {file}\x1b[0m");
                    *last = file.clone();
                }
                let mark = match step.status {
                    UiStepStatus::Passed => "\x1b[32m✔\x1b[0m",
                    UiStepStatus::Failed => "\x1b[31m✘\x1b[0m",
                    UiStepStatus::Skipped => "\x1b[33m⇣\x1b[0m",
                };
                let detail = match (&step.detail, step.status) {
                    (Some(d), UiStepStatus::Failed) => format!("\n    \x1b[31m{d}\x1b[0m"),
                    _ => String::new(),
                };
                eprintln!("  {mark} \x1b[2m#{}\x1b[0m {}  \x1b[2m{}ms\x1b[0m{detail}", step.index, step.title, step.duration_ms);
            }
            runner::RunEvent::LoopProgress { file, text, .. } => {
                if *last != *file {
                    eprintln!("\x1b[2m▶ {file}\x1b[0m");
                    *last = file.clone();
                }
                eprintln!("  \x1b[36m↻\x1b[0m {text}");
            }
            _ => {}
        }
    })
}

pub(crate) fn run_source() -> &'static str {
    if std::env::var_os("CI").is_some() {
        "ci"
    } else {
        "cli"
    }
}
