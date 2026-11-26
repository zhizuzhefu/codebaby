mod args;
mod manage;
mod output;
mod term;
mod tools;

use args::*;
use manage::*;
use output::*;
use tools::*;

use clap::{Args, Parser, Subcommand};
use codebaby_core::varstore::{self, VarDef, VarScope};
use codebaby_core::{config, doctor, envs, fmt, guide, history, importer, inspect, protocols, report, runner, scaffold, session, validate, verify, workflow};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;


fn main() -> ExitCode {
    let cli = Cli::parse();
    // 本次运行的代理覆盖：交给 core 统一解析（与 CODEBABY_PROXY 同一入口）
    if cli.no_proxy {
        std::env::set_var(codebaby_core::engine::proxy::ENV_OVERRIDE, "off");
    } else if let Some(p) = &cli.proxy {
        std::env::set_var(codebaby_core::engine::proxy::ENV_OVERRIDE, p);
    }
    let code = match cli.cmd {
        Cmd::Init { path, base_url } => cmd_init(path.as_deref(), base_url.as_deref()),
        Cmd::New { what } => cmd_new(what),
        Cmd::Grpc { action } => cmd_grpc(action),
        Cmd::Web { action } => cmd_web(action),
        Cmd::Android { action } => cmd_android(action),
        Cmd::Desktop { action } => cmd_desktop(action),
        Cmd::Proxy { action } => cmd_proxy(action),
        Cmd::About => {
            println!("{}", codebaby_core::about::text());
            0
        }
        Cmd::Upgrade { check } => cmd_upgrade(check),
        Cmd::ApplyUpdate { pid, app, staged, version, relaunch, wait_secs } => {
            match codebaby_core::update_app::apply(&codebaby_core::update_app::ApplyArgs { pid, app, staged, version, relaunch, wait_secs }) {
                Ok(()) => 0,
                Err(_) => 1,
            }
        }
        Cmd::Env { action } => cmd_env(action.unwrap_or(EnvCmd::List { json: false })),
        Cmd::Case { action: CaseCmd::New(a) } => cmd_new(NewCmd::Case(a)),
        Cmd::Case { action } => cmd_item(action.into()),
        Cmd::Collection { action: CollectionCmd::New(a) } => cmd_new(NewCmd::Collection(a)),
        Cmd::Collection { action } => cmd_item(action.into()),
        Cmd::Fmt { paths, check, fix } => cmd_fmt(&paths, check, fix),
        Cmd::Doctor { env, json } => cmd_doctor(env.as_deref(), json),
        Cmd::Guide { init: true, update, .. } => cmd_guide_init(update),
        Cmd::Guide { topic, init: false, .. } => match guide::topic(topic.as_deref().unwrap_or_default()) {
            Some(text) => {
                println!("{text}");
                0
            }
            None => {
                eprintln!("没有主题 {}，可选：\n{}", topic.unwrap_or_default(), guide::topic_list());
                2
            }
        },
        Cmd::Schema { all } => {
            if all {
                println!(
                    "{}",
                    serde_json::json!({
                        "case": serde_json::from_str::<serde_json::Value>(validate::CASE_SCHEMA).unwrap(),
                        "collection": serde_json::from_str::<serde_json::Value>(validate::COLLECTION_SCHEMA).unwrap(),
                        "env": serde_json::from_str::<serde_json::Value>(validate::ENV_SCHEMA).unwrap(),
                        "project": serde_json::from_str::<serde_json::Value>(validate::PROJECT_SCHEMA).unwrap(),
                    })
                );
            } else {
                println!("{}", validate::CASE_SCHEMA);
            }
            0
        }
        Cmd::Validate { paths, env, report } => cmd_validate(&paths, env.as_deref(), report.as_deref()),
        Cmd::List { paths } => cmd_list(&paths),
        Cmd::Vars { action: Some(a), .. } => cmd_vars_edit(a),
        Cmd::Vars { action: None, case: Some(case), env, json } => cmd_vars(&case, env.as_deref(), json),
        Cmd::Vars { action: None, case: None, json, .. } => cmd_vars_matrix(json),
        Cmd::Import { spec, curl, out, env } => cmd_import(spec.as_deref(), curl.as_deref(), out.as_deref(), env.as_deref()),
        Cmd::Send { case, env, suggest, save_body, headed } => cmd_send(&case, env.as_deref(), suggest, save_body.as_deref(), headed),
        Cmd::Workflow { action: Some(a), .. } => cmd_workflow_manage(a),
        Cmd::Workflow { action: None, name: None, .. } => cmd_workflow_manage(WorkflowCmd::List { json: false }),
        Cmd::Workflow { action: None, name: Some(name), env, dry_run, verbose, report, json_stream, no_history, no_needs } => cmd_workflow(WorkflowArgs {
            no_needs,
            name,
            env,
            dry_run,
            verbose,
            report,
            json_stream,
            no_history,
        }),
        Cmd::Verify { env, workflow, required_cases, plan, report, output, json_stream, no_history } => {
            let selection = verify::Selection { workflow, required_cases };
            if plan {
                cmd_verify_plan(env.as_deref(), report.as_deref(), &selection)
            } else {
                cmd_verify(env, report.as_deref(), output, json_stream, no_history, selection)
            }
        }
        Cmd::Watch { paths, env, tag, name } => cmd_watch(&paths, env.as_deref(), tag, name.as_deref()),
        Cmd::Run { paths, env, tag, name, failed, fail_fast, dry_run, verbose, output, parallel, report, json_stream, no_history, headed } => {
            let req = session::RunRequest { paths, env, tags: tag, name, failed, fail_fast, dry_run, verbose, parallel, output, headed };
            cmd_run(req, report.as_deref(), json_stream, no_history)
        }
        Cmd::Export { id, env, collection, format, output } => cmd_export(id, env, collection, &format, output),
        Cmd::Open { path } => cmd_open(path.as_deref()),
        Cmd::History { action: Some(a), .. } => cmd_history_clean(a),
        Cmd::History { action: None, limit, json } => cmd_history(limit, json),
    };
    ExitCode::from(code)
}

fn err(e: impl std::fmt::Display) -> u8 {
    eprintln!("\x1b[31m错误: {e}\x1b[0m");
    1
}

/// 无法执行（配置 / 路径 / 环境）：退出码 2，与「用例失败」(1) 区分
fn fatal(e: impl std::fmt::Display) -> u8 {
    err(e);
    2
}





fn cmd_run(req: session::RunRequest, report: Option<&str>, json_stream: bool, no_history: bool) -> u8 {
    let fmt = report.unwrap_or("term");
    if let Err(e) = check_report_format(fmt) {
        return err(e);
    }
    let mut spec = req.to_spec(std::path::Path::new("."), run_source(), false, None);
    spec.record_history = !no_history;
    // --failed 且历史里没有失败：没有要重跑的，属于好消息，不当错误
    if req.failed {
        if let Some(root) = spec.paths.first().and_then(|p| session::project_root(p)) {
            let env = session::resolve_env(&spec.paths, spec.env.as_deref());
            if codebaby_core::history::failed_files(&root, &env).is_empty() {
                println!("\x1b[32m✔\x1b[0m 环境 {env} 没有最近失败的用例，无需重跑");
                return 0;
            }
        }
    }
    let hook = run_hook(json_stream, fmt == "term");
    let quiet_header = json_stream || fmt == "json" || fmt == "junit";
    let dry_run = req.dry_run;
    let outcome = session::run(&spec, hook, |env, planned, timeout_ms| {
        if !quiet_header {
            println!(
                "\x1b[1mcodebaby\x1b[0m \x1b[2menv={env} cases={planned} timeout={timeout_ms}ms{}\x1b[0m",
                if dry_run { " [dry-run]" } else { "" }
            );
        }
    });
    let out = match outcome {
        Ok(o) => o,
        Err(e) => return fatal(format!("{e:#}")),
    };
    if !json_stream {
        emit_report(fmt, &out.results, &out.report, req.verbose || req.dry_run);
        term::print_notices(&out.notices);
    }
    if out.report.failed > 0 { 1 } else { 0 }
}

struct WorkflowArgs {
    no_needs: bool,
    name: String,
    env: Option<String>,
    dry_run: bool,
    verbose: bool,
    report: Option<String>,
    json_stream: bool,
    no_history: bool,
}

fn cmd_workflow(a: WorkflowArgs) -> u8 {
    let fmt = a.report.as_deref().unwrap_or("term");
    if let Err(e) = check_report_format(fmt) {
        return err(e);
    }
    let Some(root) = session::project_root(std::path::Path::new(".")) else {
        return err("未找到项目根（codebaby.json）");
    };
    let spec = workflow::WorkflowSpec {
        root,
        name: a.name.clone(),
        env: a.env.clone(),
        dry_run: a.dry_run,
        verbose: a.verbose,
        send_mode: false,
        source: run_source().into(),
        record_history: !a.no_history,
        cancel: None,
        artifacts: None,
        skip_needs: a.no_needs,
        base: None,
    };
    let is_term = fmt == "term" && !a.json_stream;
    let hook = run_hook(a.json_stream, fmt == "term");
    let verbose = a.verbose || a.dry_run;
    let mut header_done = false;
    let (name, dry_run) = (a.name.clone(), a.dry_run);
    let outcome = workflow::run(&spec, hook, |phase| {
        if !is_term {
            return;
        }
        match phase {
            workflow::StepPhase::Started(info) => {
                if !header_done {
                    header_done = true;
                    println!(
                        "\x1b[1mworkflow\x1b[0m \x1b[1m{name}\x1b[0m \x1b[2msteps={}{}\x1b[0m",
                        info.total,
                        if dry_run { " [dry-run]" } else { "" }
                    );
                }
                let mut extra = String::new();
                if !info.tags.is_empty() {
                    extra.push_str(&format!(" --tag {}", info.tags.join(",")));
                }
                if !info.name_filter.is_empty() {
                    extra.push_str(&format!(" --name {}", info.name_filter));
                }
                println!("\x1b[2m── step {}/{}: {}{} ({})\x1b[0m", info.index + 1, info.total, info.label, extra, info.planned);
            }
            workflow::StepPhase::Finished(_, results) => term::print_results(results, verbose),
        }
    });
    let out = match outcome {
        Ok(o) => o,
        Err(e) => return fatal(format!("{e:#}")),
    };
    if a.json_stream {
        return if out.report.failed > 0 { 1 } else { 0 };
    }
    if let Some(ix) = out.failed_step {
        if is_term {
            term::print_summary(&out.results, out.report.compare.as_ref());
            term::print_notices(&out.notices);
            eprintln!("\x1b[31m流程在步骤 {}/{} 失败中止\x1b[0m", ix + 1, out.steps.len());
        } else {
            emit_report(fmt, &out.results, &out.report, verbose);
        }
        return 1;
    }
    if is_term {
        term::print_summary(&out.results, out.report.compare.as_ref());
    } else {
        emit_report(fmt, &out.results, &out.report, verbose);
    }
    term::print_notices(&out.notices);
    if out.report.failed > 0 { 1 } else { 0 }
}


fn cmd_verify_plan(env: Option<&str>, report: Option<&str>, selection: &verify::Selection) -> u8 {
    let root = match project_root_or_err() {
        Ok(root) => root,
        Err(code) => return code,
    };
    let fmt = report.unwrap_or("term");
    if !matches!(fmt, "term" | "json") {
        return fatal(format!("不支持的报告格式 {fmt}（verify --plan 可选 term / json）"));
    }
    let plan = match verify::preview(&root, env, selection) {
        Ok(plan) => plan,
        Err(e) => return fatal(format!("{e:#}")),
    };
    if fmt == "json" {
        match serde_json::to_string_pretty(&plan) {
            Ok(json) => println!("{json}"),
            Err(e) => return fatal(e),
        }
    } else {
        term::print_verify_plan(&plan);
    }
    if plan.blocked.is_some() { 2 } else { 0 }
}

fn cmd_verify(env: Option<String>, report: Option<&str>, output: Option<String>, json_stream: bool, no_history: bool, selection: verify::Selection) -> u8 {
    let fmt = report.unwrap_or("term");
    if !matches!(fmt, "term" | "json" | "junit") {
        return fatal(format!("不支持的报告格式 {fmt}（verify 可选 term / json / junit）"));
    }
    let root = match project_root_or_err() {
        Ok(r) => r,
        Err(_) => return 2,
    };
    let spec = verify::VerifySpec {
        root,
        env,
        selection,
        output,
        send_mode: false,
        source: run_source().into(),
        record_history: !no_history,
        cancel: None,
    };
    let is_term = fmt == "term" && !json_stream;
    let outcome = verify::run(&spec, run_hook(json_stream, is_term), |phase| {
        if is_term {
            term::print_verify_stage(phase);
        }
    });
    let out = match outcome {
        Ok(o) => o,
        Err(e) => return fatal(format!("{e:#}")),
    };
    if json_stream {
        return out.exit_code();
    }
    if fmt == "json" {
        println!("{}", report::render_json(&out.report));
    } else if fmt == "junit" {
        print!("{}", report::render_junit(&out.report));
        term::print_notices(&out.notices);
    } else {
        term::print_notices(&out.notices);
        term::print_verify_verdict(&out);
    }
    out.exit_code()
}

/// `send --suggest`：真实响应 → 可直接粘贴的 expect / capture（与 Desktop 响应面板「字段」页同一实现）
fn print_suggestions(status: &str, body: &str) {
    let fields = codebaby_core::suggest::fields(body, 3, 60);
    let (dim, reset) = ("\x1b[2m", "\x1b[0m");
    if fields.is_empty() {
        println!("{dim}── 建议断言：响应体不是 JSON，可断言状态码与整个响应体{reset}");
        println!("  \"expect\": {{\"status\": {status}, \"body\": {{\"$\": {{\"contains\": \"…\"}}}}}}");
        return;
    }
    println!("{dim}── 建议断言：粘贴进用例的 \"expect\": {{\"status\": {status}, \"body\": {{ … }}}}（值每次会变的 id / 时间 / token 用右侧的类型断言）{reset}");
    for f in &fields {
        let alt = f.type_expect().snippet();
        let alt = alt.split_once(": ").map(|(_, m)| m).unwrap_or_default();
        println!("  {:<56} {dim}// 或 {alt}{reset}", format!("{},", f.value_expect().snippet()));
    }
    println!("{dim}── 建议捕获：粘贴进 \"capture\": {{ … }}，后续用例用 {{{{名称}}}} 引用{reset}");
    for f in fields.iter().filter(|f| !matches!(f.value, serde_json::Value::Object(_) | serde_json::Value::Array(_))).take(12) {
        println!("  {},", f.capture(&f.var_name()).snippet());
    }
}

fn cmd_send(case: &str, env: Option<&str>, suggest: bool, save_body: Option<&str>, headed: bool) -> u8 {
    let path = PathBuf::from(case);
    if !path.exists() {
        return err(format!("用例不存在 {case}"));
    }
    let mut spec = session::send_spec(path, case, env.map(String::from), run_source(), None);
    spec.opts.headed = headed;
    let out = match session::run(&spec, None, |_, _, _| {}) {
        Ok(o) => o,
        Err(e) => return err(format!("{e:#}")),
    };
    // 自动补跑的前置用例排在前面：只汇报它们的结论，详情看目标用例
    let Some(res) = out.results.iter().find(|r| !out.prerequisites.contains(&r.rel)) else {
        return err("未发现用例");
    };

    let cyan = "\x1b[36m";
    let green = "\x1b[32m";
    let dim = "\x1b[2m";
    let bold = "\x1b[1m";
    let reset = "\x1b[0m";
    for n in out.notices.iter().filter(|n| n.starts_with("先运行前置用例")) {
        println!("{dim}↪ {n}{reset}");
    }
    for p in out.results.iter().filter(|r| out.prerequisites.contains(&r.rel)) {
        let mark = if p.passed() { format!("{green}✔{reset}") } else { "\x1b[31m✘\x1b[0m".to_string() };
        println!("  {mark} {dim}{} {}{reset}{}", p.status_text(), p.rel, p.error.as_ref().map(|e| format!("  {e}")).unwrap_or_default());
    }
    println!("{bold}{cyan}{} {reset}{bold}{}{reset}", res.method, res.url);
    if let Some(e) = &res.error {
        println!("\x1b[31m✘ {e}{reset}");
    }
    if let Some(resp) = &res.full_response {
        use codebaby_core::config::Protocol;
        let code = if res.status_is_bad() { "\x1b[31m" } else { green };
        println!("{dim}──{reset} 状态 {code}{bold}{}{reset} {dim}· {}ms · {} bytes{reset}", res.status_text(), resp.time_ms, resp.size);
        let (head_title, body_title) = match resp.protocol {
            Protocol::Http => ("响应头", "响应体"),
            Protocol::Grpc => ("响应元数据", "响应消息"),
            Protocol::Ws if resp.status != 101 => ("握手响应头", "握手响应体"),
            Protocol::Ws => ("握手响应头", "收到的消息"),
            Protocol::Web => ("页面响应头", "结束时的页面"),
            Protocol::Android => ("", "结束时的界面"),
            Protocol::Desktop => ("", "结束时的窗口"),
        };
        // 界面用例：步骤时间线（每一步的截图）+ 运行环境 + 失败现场
        if let Some(ev) = &resp.evidence {
            println!("{dim}──{reset} 步骤 {dim}（截图在 {}）{reset}", ev.dir);
            term::print_ui_steps(ev, "  ");
            if !ev.environment.is_empty() {
                println!("{dim}── 环境 {}{reset}", ev.environment.iter().filter(|(k, _)| k != "User-Agent").map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(" · "));
            }
            for a in &ev.artifacts {
                println!("{dim}── 失败现场 {}：{}/{}{reset}", a.label, ev.dir, a.file);
            }
        }
        if !resp.headers.is_empty() && !resp.protocol.is_ui() {
            println!("{dim}──{reset} {head_title}");
            for (k, v) in &resp.headers {
                println!("  {cyan}{k}{reset}{dim}:{reset} {v}");
            }
        }
        // WebSocket / gRPC 流：按时间顺序的消息（→ 发出 ← 收到）
        if resp.protocol == Protocol::Ws && !resp.frames.is_empty() {
            println!("{dim}──{reset} 对话");
            for f in &resp.frames {
                let arrow = if f.sent { format!("{cyan}→{reset}") } else { format!("{green}←{reset}") };
                let kind = if f.kind == "text" { String::new() } else { format!("{dim}[{}]{reset} ", f.kind) };
                println!("  {arrow} {dim}{:>6}ms{reset} {kind}{}", f.at_ms, f.data);
            }
        }
        println!("{dim}──{reset} {body_title}");
        match &res.raw_body {
            Some(raw) => println!("{dim}<二进制响应 {} 字节；用 --save-body <文件> 保存>{reset}", raw.len()),
            None => println!("{}", pretty_json_colored(&resp.body)),
        }
        if let Some(file) = save_body {
            let bytes: &[u8] = res.raw_body.as_deref().map(|b| b.as_slice()).unwrap_or(resp.body.as_bytes());
            match codebaby_core::fsx::write(file, bytes) {
                Ok(()) => println!("{green}✔{reset} 响应体已保存 {file}（{} 字节）", bytes.len()),
                Err(e) => eprintln!("\x1b[31m保存失败 {file}: {e}{reset}"),
            }
        }
        if suggest {
            let status = if resp.protocol == codebaby_core::config::Protocol::Grpc { format!("\"{}\"", res.status_text()) } else { resp.status.to_string() };
            print_suggestions(&status, &resp.body);
        }
    }
    if !res.tests.is_empty() {
        let passed = res.tests.iter().filter(|t| t.pass).count();
        println!("{dim}──{reset} 断言 {green}{passed}{reset}/{dim}{}{reset}", res.tests.len());
        for t in &res.tests {
            let sym = if t.pass { format!("{green}✔{reset}") } else { format!("\x1b[31m✘{reset}") };
            println!("  {sym} {}", t.expr);
        }
    }
    if !res.logs.is_empty() {
        println!("{dim}──{reset} 日志");
        for l in &res.logs {
            println!("  {dim}{l}{reset}");
        }
    }
    if res.passed() { 0 } else { 1 }
}


fn cmd_fmt(paths: &[String], check: bool, fix: bool) -> u8 {
    let paths: Vec<String> = if paths.is_empty() { vec!["collections".into()] } else { paths.to_vec() };
    let pb: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    let report = match fmt::format_paths(&pb, check, fix) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("\x1b[31m错误: {e:#}\x1b[0m");
            return 2;
        }
    };
    for c in &report.changes {
        match c {
            fmt::FmtChange::Case(f) if check => println!("\x1b[33m可精简\x1b[0m {}", f.display()),
            fmt::FmtChange::Case(f) => println!("\x1b[32m已格式化\x1b[0m {}", f.display()),
            fmt::FmtChange::HoistedHeaders(f) => println!("\x1b[32m已上提 headers\x1b[0m {}", f.display()),
            fmt::FmtChange::Upgraded(f, applied) => {
                println!("{} {}", if check { "\x1b[33m可改写\x1b[0m" } else { "\x1b[32m已改写\x1b[0m" }, f.display());
                for a in applied {
                    println!("  \x1b[2m· {a}\x1b[0m");
                }
            }
        }
    }
    if report.case_changes() == 0 {
        println!("\x1b[32m✔ 已是精简形态\x1b[0m");
    }
    if check && report.case_changes() > 0 { 1 } else { 0 }
}

fn cmd_doctor(env: Option<&str>, json: bool) -> u8 {
    let r = doctor::run(std::path::Path::new("."), env);
    if json {
        println!("{}", serde_json::to_string_pretty(&r).unwrap_or_default());
    } else {
        println!("\x1b[1mcodebaby doctor\x1b[0m");
        for c in &r.checks {
            let mark = match c.level {
                doctor::CheckLevel::Ok => "\x1b[32m✔\x1b[0m",
                doctor::CheckLevel::Warn => "\x1b[33m⚠\x1b[0m",
                doctor::CheckLevel::Error => "\x1b[31m✘\x1b[0m",
            };
            println!("  {mark} {}", c.message);
        }
    }
    if r.ok() { 0 } else { 2 }
}

fn default_env_of(paths: &[PathBuf]) -> Option<String> {
    session::default_env_of(paths)
}

fn cmd_validate(paths: &[String], env: Option<&str>, report: Option<&str>) -> u8 {
    let json = report.map(|r| r == "json").unwrap_or(false);
    let explicit: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    let probe = if explicit.is_empty() { vec![PathBuf::from(".")] } else { explicit.clone() };
    let env = env
        .map(String::from)
        .or_else(|| default_env_of(&probe))
        .unwrap_or_else(|| "dev".into());
    // 不带路径：collections + 本环境会运行的流程引用的目录（与 verify / 桌面端校验面板同一范围）
    let pb = if explicit.is_empty() {
        match session::project_root(std::path::Path::new(".")) {
            Some(root) => validate::project_scope(&root, &env),
            None => vec![PathBuf::from("collections")],
        }
    } else {
        explicit
    };
    match validate::validate_paths(&pb, &env) {
        Ok(issues) => {
            let errors = issues.iter().filter(|i| i.level == "error").count();
            let warns = issues.len() - errors;
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "ok": errors == 0,
                        "errors": errors,
                        "warnings": warns,
                        "issues": issues,
                    })
                );
                return if errors > 0 { 2 } else { 0 };
            }
            for i in &issues {
                let (color, sym) = if i.level == "error" { ("\x1b[31m", "✘") } else { ("\x1b[33m", "⚠") };
                println!("{color}{sym}\x1b[0m {file} [{rule}]: {msg}", sym = sym, color = color, file = i.file, rule = i.rule, msg = i.message);
            }
            if issues.is_empty() {
                println!("\x1b[32m✔ 全部通过\x1b[0m");
                0
            } else {
                println!("\n{} 错误, {} 警告", errors, warns);
                let fixable = issues.iter().filter(|i| i.fixable).count();
                if fixable > 0 {
                    println!("\x1b[2m{fixable} 条写法建议可自动改写（语义不变）：codebaby fmt --fix [目录]\x1b[0m");
                }
                if errors > 0 { 2 } else { 0 }
            }
        }
        Err(e) => {
            eprintln!("校验失败: {e:#}");
            2
        }
    }
}

fn cmd_list(paths: &[String]) -> u8 {
    let paths: Vec<String> = if paths.is_empty() { vec!["collections".into()] } else { paths.to_vec() };
    let pb: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    match runner::discover::discover(&pb) {
        Ok(cases) => {
            for dc in &cases {
                let chain: Vec<String> = dc.chain.iter().map(|c| c.name.clone()).collect();
                println!(
                    "\x1b[36m{}\x1b[0m  \x1b[2m[{}]\x1b[0m",
                    dc.rel,
                    chain.join(" / ")
                );
            }
            println!("\n共 {} 个用例", cases.len());
            0
        }
        Err(e) => {
            eprintln!("{e:#}");
            1
        }
    }
}


fn cmd_import(spec: Option<&str>, curl: Option<&str>, out: Option<&str>, env: Option<&str>) -> u8 {
    let out_dir = std::path::PathBuf::from(out.unwrap_or("imported"));
    let env_name = env.unwrap_or("dev");
    // curl 命令模式：单用例导入
    if let Some(cmdline) = curl {
        return match importer::import_curl(cmdline) {
            Ok(case) => {
                let dir = out_dir.join("collections");
                let _ = std::fs::create_dir_all(&dir);
                let file = dir.join("01-imported-from-curl.json");
                codebaby_core::fsx::write(&file, serde_json::to_vec_pretty(&case).unwrap_or_default()).ok();
                println!("\x1b[32m✔\x1b[0m curl 命令已导入: {}", file.display());
                0
            }
            Err(e) => {
                eprintln!("\x1b[31m导入失败: {e:#}\x1b[0m");
                2
            }
        };
    }
    let Some(spec) = spec else {
        eprintln!("\x1b[31m错误: 需要 spec 文件或 --curl 命令\x1b[0m");
        return 2;
    };
    match importer::import(std::path::Path::new(spec), &out_dir, env_name) {
        Ok(sum) => {
            println!(
                "\x1b[32m✔\x1b[0m 导入完成: {} 个用例 / {} 个集合 → {}（baseUrl: {}）",
                sum.cases,
                sum.collections,
                sum.out_dir.display(),
                sum.base_url
            );
            for w in &sum.warnings {
                println!("\x1b[33m⚠\x1b[0m {w}");
            }
            println!("\n下一步: cd {} && codebaby run collections --dry-run", sum.out_dir.display());
            0
        }
        Err(e) => {
            eprintln!("\x1b[31m导入失败: {e:#}\x1b[0m");
            1
        }
    }
}

fn cmd_watch(paths: &[String], env: Option<&str>, tags: Vec<String>, name: Option<&str>) -> u8 {
    let paths: Vec<String> = if paths.is_empty() { vec!["collections".into()] } else { paths.to_vec() };
    let pb: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    // watch 目标 = 用例 + 项目/集合/环境配置
    let mut watch_files: Vec<PathBuf> = pb.clone();
    if let Ok(cases) = runner::discover::discover(&pb) {
        watch_files.extend(cases.iter().map(|c| c.path.clone()));
    }
    if let Some(root) = runner::discover::find_project_root(&pb[0]) {
        watch_files.extend(runner::discover::project_file(&root));
        let envs = root.join("envs");
        if let Ok(rd) = std::fs::read_dir(&envs) {
            for e in rd.flatten() {
                watch_files.push(e.path());
            }
        }
        if let Ok(rd) = std::fs::read_dir(root.join("collections")) {
            for e in rd.flatten() {
                if e.path().join("collection.json").exists() {
                    watch_files.push(e.path().join("collection.json"));
                }
            }
        }
    }

    eprintln!("\x1b[2mwatching {} 个文件，变更自动重跑（Ctrl-C 退出）\x1b[0m", watch_files.len());
    let mut args: Vec<String> = vec!["run".into()];
    args.extend(pb.iter().map(|p| p.display().to_string()));
    if let Some(e) = env {
        args.push("-e".into());
        args.push(e.into());
    }
    for t in tags {
        args.push("--tag".into());
        args.push(t);
    }
    if let Some(n) = name {
        args.push("--name".into());
        args.push(n.into());
    }
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("codebaby"));
    loop {
        print!("\x1b[2J\x1b[H");
        let _ = std::process::Command::new(&exe).args(&args).status();
        eprintln!("\n\x1b[2m── 已完成，等待变更…\x1b[0m");
        // 事件驱动等待：任一 watch 文件变更/新建即触发重跑
        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |res: Result<notify::Event, _>| {
            if res.is_ok() {
                let _ = tx.send(());
            }
        })
        .expect("文件监听初始化失败");
        use notify::Watcher;
        for f in &watch_files {
            let _ = watcher.watch(f, notify::RecursiveMode::NonRecursive);
        }
        // 父目录监听捕获新建文件（新增用例）
        for f in watch_files.iter().filter_map(|p| p.parent()).collect::<std::collections::BTreeSet<_>>() {
            let _ = watcher.watch(f, notify::RecursiveMode::NonRecursive);
        }
        let _ = rx.recv();
    }
}

/// 桌面端可执行文件：与 CLI 同目录优先，其次 PATH
/// 查找桌面端：同目录（开发构建）→ macOS 已安装的 codebaby.app → PATH
fn desktop_exe() -> Option<PathBuf> {
    let name = if cfg!(windows) { "codebaby-desktop.exe" } else { "codebaby-desktop" };
    if let Some(sibling) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(name))) {
        if sibling.is_file() {
            return Some(sibling);
        }
    }
    if cfg!(target_os = "macos") {
        let bundle = PathBuf::from("Applications/codebaby.app/Contents/MacOS").join(name);
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let app = [Some(PathBuf::from("/")), home].into_iter().flatten().map(|base| base.join(&bundle)).find(|p| p.is_file());
        if app.is_some() {
            return app;
        }
    }
    std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|d| d.join(name)).find(|p| p.is_file()))
}

fn cmd_open(path: Option<&str>) -> u8 {
    let dir = PathBuf::from(path.unwrap_or("."));
    let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
    let root = session::project_root(&dir).unwrap_or(dir);
    let Some(exe) = desktop_exe() else {
        eprintln!(
            "\x1b[33m未找到 codebaby-desktop。\x1b[0m macOS：安装 codebaby.app 到 /Applications（scripts/build-mac-release.sh）；其他平台：cargo install --path crates/codebaby-desktop"
        );
        return 1;
    };
    match std::process::Command::new(&exe).arg(&root).spawn() {
        Ok(_) => {
            println!("\x1b[32m✔\x1b[0m 已在桌面端打开 {}", root.display());
            0
        }
        Err(e) => {
            eprintln!("\x1b[31m启动桌面端失败: {e}\x1b[0m");
            1
        }
    }
}

fn cmd_export(id: Option<String>, env: Option<String>, collection: Vec<String>, format: &str, output: Option<String>) -> u8 {
    use codebaby_core::export;
    let root = match project_root_or_err() {
        Ok(r) => r,
        Err(code) => return code,
    };
    let format = match export::Format::parse(format) {
        Ok(f) => f,
        Err(e) => return fatal(e),
    };
    if id.is_some() && !collection.is_empty() {
        return fatal("运行 ID 与 --collection 不能同时指定：--collection 取的是该集合每个用例的最新结果");
    }
    let collection: Vec<String> = collection.iter().map(|c| export::normalize_collection(&root, c)).collect();
    let scope = match id {
        Some(id) => export::Scope::Run(id),
        None => export::Scope::Latest { env: session::resolve_env(std::slice::from_ref(&root), env.as_deref()), collections: collection.clone() },
    };
    let report = match export::collect(&root, &scope) {
        Ok(r) => r,
        Err(e) => return fatal(format!("{e:#}")),
    };
    let name = export::file_name(&root, &report, format);
    let path = match output.map(PathBuf::from) {
        Some(p) if p.is_dir() => p.join(&name),
        Some(p) => p,
        None => PathBuf::from(&name),
    };
    if let Err(e) = codebaby_core::fsx::write(&path, export::render(&report, &root, format)) {
        return err(format!("写入 {} 失败: {e:#}", path.display()));
    }
    let overview = export::Overview::of(&report, &root, &collection);
    println!("\x1b[32m✔\x1b[0m 已导出 {}（{}）", path.display(), overview.headline());
    for note in overview.notes() {
        println!("\x1b[33m!\x1b[0m {note}");
    }
    0
}

fn cmd_history(limit: usize, json: bool) -> u8 {
    let Some(root) = session::project_root(std::path::Path::new(".")) else {
        eprintln!("\x1b[31m错误: 未找到项目根（codebaby.json）\x1b[0m");
        return 1;
    };
    let entries: Vec<history::RunEntry> = history::list(&root).into_iter().take(limit).collect();
    if json {
        let reports: Vec<&report::RunReport> = entries.iter().map(|e| &e.report).collect();
        println!("{}", serde_json::to_string_pretty(&reports).unwrap_or_default());
        return 0;
    }
    if entries.is_empty() {
        println!("\x1b[2m暂无运行历史（{}）\x1b[0m", history::runs_dir(&root).display());
        return 0;
    }
    for e in &entries {
        let r = &e.report;
        let m = r.meta.clone().unwrap_or_default();
        let mark = if r.failed > 0 { "\x1b[31m✘\x1b[0m" } else { "\x1b[32m✔\x1b[0m" };
        println!(
            "{mark} \x1b[2m{}\x1b[0m {:<8} env={:<6} {}/{} passed  \x1b[2m{}\x1b[0m",
            m.id, m.source, m.env, r.passed, r.total, m.command
        );
    }
    0
}

fn cmd_history_clean(action: HistoryCmd) -> u8 {
    let root = match project_root_or_err() {
        Ok(r) => r,
        Err(_) => return 2,
    };
    let r: anyhow::Result<String> = (|| match action {
        HistoryCmd::Rm { ids } => {
            for id in &ids {
                history::delete(&root, id)?;
            }
            Ok(format!("已删除 {} 条运行记录", ids.len()))
        }
        HistoryCmd::Clear { all: false, keep: None, older_than: None } => {
            anyhow::bail!("请指定清理范围：--all（全部）/ --keep <N> / --older-than <7d>")
        }
        HistoryCmd::Clear { keep, older_than, .. } => {
            let older_than_ms = older_than.as_deref().map(history::parse_duration_ms).transpose()?;
            let n = history::clean(&root, history::Clean { keep, older_than_ms })?;
            Ok(format!("已清理 {n} 条运行记录（剩余 {} 条）", history::list_paths(&root).len()))
        }
    })();
    match r {
        Ok(msg) => {
            println!("\x1b[32m✔\x1b[0m {msg}");
            0
        }
        Err(e) => fatal(format!("{e:#}")),
    }
}

/// JSON pretty + 轻量着色；非 JSON 原样返回
fn pretty_json_colored(body: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return body.to_string();
    };
    let pretty = serde_json::to_string_pretty(&v).unwrap_or_default();
    // 轻量着色：key 青、字符串绿、数字黄
    let mut out = String::with_capacity(pretty.len() * 2);
    let mut in_str = false;
    let chars: Vec<char> = pretty.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '"' => {
                in_str = !in_str;
                if in_str {
                    // key 判定：向后看下一个非空白字符是否 ':'
                    let mut j = i + 1;
                    while j < chars.len() && chars[j] != '"' {
                        j += 1;
                    }
                    // 找配对引号后的第一个非空白
                    let mut k = j + 1;
                    while k < chars.len() && chars[k].is_whitespace() {
                        k += 1;
                    }
                    let is_key = k < chars.len() && chars[k] == ':';
                    out.push_str(if is_key { "\x1b[36m\"" } else { "\x1b[32m\"" });
                } else {
                    out.push('"');
                    out.push_str("\x1b[0m");
                }
            }
            '0'..='9' if !in_str => {
                out.push_str("\x1b[33m");
                out.push(c);
                // 数字结束判断交给下一个非数字字符重置
                let next = chars.get(i + 1);
                if !next.map(|n| n.is_ascii_digit()).unwrap_or(false) {
                    out.push_str("\x1b[0m");
                }
            }
            _ => out.push(c),
        }
    }
    out
}
