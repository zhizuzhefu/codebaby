mod term;

use clap::{Parser, Subcommand};
use requestbaby_core::{doctor, fmt, history, importer, inspect, report, runner, scaffold, session, validate, workflow};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

#[derive(Parser)]
#[command(
    name = "requestbaby",
    version,
    about = "CLI Postman: curl-powered, file-based, AI-native API testing",
    long_about = "requestbaby — 把 curl 组织成集合，用 JSON 配置做项目级接口自动化测试。\n配置规范见 docs/schema/case.schema.json（requestbaby schema 导出）。"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 初始化项目脚手架
    Init {
        /// 目标目录（缺省当前目录）
        path: Option<String>,
    },
    /// 新建用例 / 集合 / 环境（与 Desktop 新建同一模板）
    New {
        #[command(subcommand)]
        what: NewCmd,
    },
    /// 格式化并精简用例：重复 headers 上提集合级、字段规范排序、清理空字段
    Fmt {
        /// 目标目录（缺省 collections）
        paths: Vec<String>,
        /// 只检查不修改（CI 用：有可精简项则 exit 1 并列出）
        #[arg(long)]
        check: bool,
    },
    /// 环境自检（AI 调度前置）：引擎 / 配置完整性 / 环境 / 用例 / 历史
    Doctor {
        /// 输出 JSON
        #[arg(long)]
        json: bool,
    },
    /// 导出 JSON Schema（--all 输出 case/collection/env/project bundle）
    Schema {
        /// 全部四类
        #[arg(long)]
        all: bool,
    },
    /// 校验配置：Schema 结构 + 变量引用 + 脚本语法 + 引用存在性
    Validate {
        /// 要校验的文件/目录（缺省 collections）
        paths: Vec<String>,
        /// 环境名（决定参与变量检查的 env 文件）
        #[arg(short, long)]
        env: Option<String>,
        /// 报告格式：term（默认）/ json（file/level/rule/message 四元组）
        #[arg(long)]
        report: Option<String>,
    },
    /// 列出可执行的用例
    List {
        paths: Vec<String>,
    },
    /// 变量解析预览：用例最终使用的变量值、来源层级、运行时捕获与未定义引用
    Vars {
        /// 用例文件
        case: String,
        #[arg(short, long)]
        env: Option<String>,
        /// 输出 JSON
        #[arg(long)]
        json: bool,
    },
    /// 导入外部格式：OpenAPI 3 / Swagger 2 / Postman Collection v2.1（JSON 或 YAML）
    Import {
        /// spec 文件路径（OpenAPI/Swagger/Postman JSON 或 YAML）
        spec: Option<String>,
        /// 直接导入 curl 命令（与 spec 二选一）
        #[arg(long)]
        curl: Option<String>,
        /// 输出目录（生成完整项目脚手架，缺省 ./imported）
        #[arg(short, long)]
        out: Option<String>,
        /// 环境名（缺省 dev）
        #[arg(short, long)]
        env: Option<String>,
    },
    /// 交互式发送单个用例：展示状态/响应头/美化响应体/断言（调试用）
    Send {
        /// 用例文件
        case: String,
        /// 环境名
        #[arg(short, long)]
        env: Option<String>,
    },
    /// 运行命名工作流（多阶段编排，变量跨阶段自动传递）
    /// 定义在 requestbaby.json 的 workflows 字段
    Workflow {
        /// 工作流名
        name: String,
        #[arg(short, long)]
        env: Option<String>,
        /// 编译 curl 不执行
        #[arg(long)]
        dry_run: bool,
        #[arg(short, long)]
        verbose: bool,
        /// 报告格式：term / json / html
        #[arg(long)]
        report: Option<String>,
        /// 逐行输出运行事件（NDJSON，含 stepStarted / stepFinished）
        #[arg(long)]
        json_stream: bool,
        /// 不写入运行历史
        #[arg(long)]
        no_history: bool,
    },
    /// 监听配置变更自动重跑（开发时挂终端）
    Watch {
        paths: Vec<String>,
        #[arg(short, long)]
        env: Option<String>,
        #[arg(long)]
        tag: Vec<String>,
        #[arg(long)]
        name: Option<String>,
    },
    /// 运行用例 / 集合
    Run {
        /// 文件 / 目录
        paths: Vec<String>,
        /// 环境名（缺省用 requestbaby.json 的 defaultEnv，再缺省 dev）
        #[arg(short, long)]
        env: Option<String>,
        /// 标签过滤（可多次，任一命中即选）
        #[arg(long)]
        tag: Vec<String>,
        /// 用例名子串过滤
        #[arg(long)]
        name: Option<String>,
        /// 首个失败立即中止
        #[arg(long)]
        fail_fast: bool,
        /// 编译出 curl 命令打印，不执行
        #[arg(long)]
        dry_run: bool,
        /// 详细输出：curl 命令 / 全部断言 / 脚本日志
        #[arg(short, long)]
        verbose: bool,
        /// 响应落盘目录（每用例一个 JSON：meta + body）
        #[arg(short, long)]
        output: Option<String>,
        /// 独立用例并发档位（依赖捕获链/带脚本的仍串行在前；不支持 setup/teardown）
        #[arg(long)]
        parallel: Option<usize>,
        /// 报告格式：term（默认）/ json / html（写 report.html）
        #[arg(long)]
        report: Option<String>,
        /// 逐行输出运行事件（NDJSON，与 Desktop 同一事件协议），替代常规报告
        #[arg(long)]
        json_stream: bool,
        /// 不写入运行历史（.requestbaby/runs/）
        #[arg(long)]
        no_history: bool,
    },
    /// 在桌面端打开项目（需安装 requestbaby-desktop）
    Open {
        /// 项目目录（缺省当前目录）
        path: Option<String>,
    },
    /// 查看运行历史（CLI / Desktop 共享 .requestbaby/runs/）
    History {
        /// 显示条数
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: usize,
        /// 输出 JSON（RunReport 数组）
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum NewCmd {
    /// 在集合目录下新建用例（自动编号 NN-<名称>.json）
    Case {
        /// 集合目录
        dir: String,
        /// 用例名
        name: String,
        #[arg(short, long, default_value = "GET")]
        method: String,
        #[arg(short, long, default_value = "")]
        url: String,
    },
    /// 新建集合目录（含 collection.json）
    Collection {
        /// 父目录
        parent: String,
        name: String,
    },
    /// 新建环境 envs/<name>.json
    Env {
        name: String,
        /// 复制已有环境的变量
        #[arg(long)]
        from: Option<String>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let code = match cli.cmd {
        Cmd::Init { path } => cmd_init(path.as_deref()),
        Cmd::New { what } => cmd_new(what),
        Cmd::Fmt { paths, check } => cmd_fmt(&paths, check),
        Cmd::Doctor { json } => cmd_doctor(json),
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
        Cmd::Vars { case, env, json } => cmd_vars(&case, env.as_deref(), json),
        Cmd::Import { spec, curl, out, env } => cmd_import(spec.as_deref(), curl.as_deref(), out.as_deref(), env.as_deref()),
        Cmd::Send { case, env } => cmd_send(&case, env.as_deref()),
        Cmd::Workflow { name, env, dry_run, verbose, report, json_stream, no_history } => cmd_workflow(WorkflowArgs {
            name,
            env,
            dry_run,
            verbose,
            report,
            json_stream,
            no_history,
        }),
        Cmd::Watch { paths, env, tag, name } => cmd_watch(&paths, env.as_deref(), tag, name.as_deref()),
        Cmd::Run { paths, env, tag, name, fail_fast, dry_run, verbose, output, parallel, report, json_stream, no_history } => {
            let req = session::RunRequest { paths, env, tags: tag, name, fail_fast, dry_run, verbose, parallel, output };
            cmd_run(req, report.as_deref(), json_stream, no_history)
        }
        Cmd::Open { path } => cmd_open(path.as_deref()),
        Cmd::History { limit, json } => cmd_history(limit, json),
    };
    ExitCode::from(code)
}

fn err(e: impl std::fmt::Display) -> u8 {
    eprintln!("\x1b[31m错误: {e}\x1b[0m");
    1
}

fn cmd_init(path: Option<&str>) -> u8 {
    let dir = PathBuf::from(path.unwrap_or("."));
    match scaffold::init_project(&dir) {
        Ok(entries) => {
            for e in entries {
                if e.created {
                    println!("创建: {}", e.path.display());
                } else {
                    eprintln!("跳过已存在: {}", e.path.display());
                }
            }
            println!("\n完成。试试: requestbaby run {}", dir.join("collections").display());
            0
        }
        Err(e) => err(format!("{e:#}")),
    }
}

fn cmd_new(what: NewCmd) -> u8 {
    let created = match what {
        NewCmd::Case { dir, name, method, url } => scaffold::new_case(&PathBuf::from(dir), &name, &method, &url),
        NewCmd::Collection { parent, name } => scaffold::new_collection(&PathBuf::from(parent), &name),
        NewCmd::Env { name, from } => match session::project_root(std::path::Path::new(".")) {
            Some(root) => scaffold::new_env(&root, &name, from.as_deref()),
            None => Err(anyhow::anyhow!("未找到项目根（requestbaby.json）")),
        },
    };
    match created {
        Ok(p) => {
            println!("\x1b[32m✔\x1b[0m 已创建 {}", p.display());
            0
        }
        Err(e) => err(format!("{e:#}")),
    }
}

/// 统一报告输出（run / workflow 共用）
fn emit_report(fmt: &str, results: &[runner::CaseResult], run_report: &report::RunReport, verbose: bool) {
    match fmt {
        "json" => {
            let mut r = report::RunReport::new(results, verbose, run_report.meta.clone());
            r.steps = run_report.steps.clone();
            println!("{}", report::render_json(&r));
        }
        "html" => {
            let path = PathBuf::from("report.html");
            std::fs::write(&path, report::render_html(run_report)).ok();
            eprintln!("\x1b[2mHTML 报告: {}\x1b[0m", path.display());
            term::print_summary(results);
        }
        _ => {
            term::print_results(results, verbose);
            term::print_summary(results);
        }
    }
}

fn check_report_format(fmt: &str) -> Result<(), String> {
    match fmt {
        "term" | "json" | "html" => Ok(()),
        other => Err(format!("不支持的报告格式 {other}（可选 term / json / html）")),
    }
}

fn json_stream_hook() -> runner::EventHook {
    Arc::new(|ev: &runner::RunEvent| {
        use std::io::Write;
        let line = serde_json::to_string(ev).unwrap_or_default();
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    })
}

fn run_source() -> &'static str {
    if std::env::var_os("CI").is_some() {
        "ci"
    } else {
        "cli"
    }
}

fn cmd_run(req: session::RunRequest, report: Option<&str>, json_stream: bool, no_history: bool) -> u8 {
    let fmt = report.unwrap_or("term");
    if let Err(e) = check_report_format(fmt) {
        return err(e);
    }
    let mut spec = req.to_spec(std::path::Path::new("."), run_source(), false, None);
    spec.record_history = !no_history;
    let hook = json_stream.then(json_stream_hook);
    let quiet_header = json_stream || fmt == "json";
    let dry_run = req.dry_run;
    let outcome = session::run(&spec, hook, |env, planned, timeout_ms| {
        if !quiet_header {
            println!(
                "\x1b[1mrequestbaby\x1b[0m \x1b[2menv={env} cases={planned} timeout={timeout_ms}ms{}\x1b[0m",
                if dry_run { " [dry-run]" } else { "" }
            );
        }
    });
    let out = match outcome {
        Ok(o) => o,
        Err(e) => return err(format!("{e:#}")),
    };
    if !json_stream {
        emit_report(fmt, &out.results, &out.report, req.verbose || req.dry_run);
        term::print_notices(&out.notices);
    }
    if out.report.failed > 0 { 1 } else { 0 }
}

struct WorkflowArgs {
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
        return err("未找到项目根（requestbaby.json）");
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
    };
    let is_term = fmt == "term" && !a.json_stream;
    let hook = a.json_stream.then(json_stream_hook);
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
        Err(e) => return err(format!("{e:#}")),
    };
    if a.json_stream {
        return if out.report.failed > 0 { 1 } else { 0 };
    }
    if let Some(ix) = out.failed_step {
        if is_term {
            term::print_summary(&out.results);
            term::print_notices(&out.notices);
            eprintln!("\x1b[31m工作流在步骤 {}/{} 失败中止\x1b[0m", ix + 1, out.steps.len());
        } else {
            emit_report(fmt, &out.results, &out.report, verbose);
        }
        return 1;
    }
    if is_term {
        term::print_summary(&out.results);
    } else {
        emit_report(fmt, &out.results, &out.report, verbose);
    }
    term::print_notices(&out.notices);
    if out.report.failed > 0 { 1 } else { 0 }
}

fn cmd_send(case: &str, env: Option<&str>) -> u8 {
    let path = PathBuf::from(case);
    if !path.exists() {
        return err(format!("用例不存在 {case}"));
    }
    let spec = session::send_spec(path, case, env.map(String::from), run_source(), None);
    let out = match session::run(&spec, None, |_, _, _| {}) {
        Ok(o) => o,
        Err(e) => return err(format!("{e:#}")),
    };
    let Some(res) = out.results.first() else {
        return err("未发现用例");
    };

    let cyan = "\x1b[36m";
    let green = "\x1b[32m";
    let dim = "\x1b[2m";
    let bold = "\x1b[1m";
    let reset = "\x1b[0m";
    println!("{bold}{cyan}{} {reset}{bold}{}{reset}", res.method, res.url);
    if let Some(e) = &res.error {
        println!("\x1b[31m✘ {e}{reset}");
    }
    if let Some(resp) = &res.full_response {
        let code = if resp.status >= 400 { "\x1b[31m" } else { green };
        println!("{dim}──{reset} 状态 {code}{bold}{}{reset} {dim}· {}ms · {} bytes{reset}", resp.status, resp.time_ms, resp.size);
        if !resp.headers.is_empty() {
            println!("{dim}──{reset} 响应头");
            for (k, v) in &resp.headers {
                println!("  {cyan}{k}{reset}{dim}:{reset} {v}");
            }
        }
        println!("{dim}──{reset} 响应体");
        println!("{}", pretty_json_colored(&resp.body));
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

fn cmd_vars(case: &str, env: Option<&str>, json: bool) -> u8 {
    let path = PathBuf::from(case);
    if !path.exists() {
        return err(format!("用例不存在 {case}"));
    }
    let env = session::resolve_env(std::slice::from_ref(&path), env);
    let p = match inspect::case_vars(&path, &env) {
        Ok(p) => p,
        Err(e) => return err(format!("{e:#}")),
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&p).unwrap_or_default());
        return if p.undefined.is_empty() { 0 } else { 2 };
    }
    let (dim, red, yellow, cyan, reset) = ("\x1b[2m", "\x1b[31m", "\x1b[33m", "\x1b[36m", "\x1b[0m");
    println!("\x1b[1m变量解析\x1b[0m {dim}env={}{reset}", p.env);
    for e in &p.entries {
        let mark = if e.used { format!("{cyan}●{reset}") } else { format!("{dim}○{reset}") };
        let over = if e.overridden.is_empty() { String::new() } else { format!(" {dim}(覆盖 {}){reset}", e.overridden.join(" → ")) };
        println!("{mark} {:<20} = {:<32} {dim}← {}{reset}{over}", e.name, e.value, e.source);
    }
    if !p.runtime.is_empty() {
        println!("{dim}── 运行时捕获（脚本写入 vars.x）{reset}");
        for r in &p.runtime {
            let mark = if r.used { format!("{cyan}●{reset}") } else { format!("{dim}○{reset}") };
            println!("{mark} {:<20} {dim}← {}{reset}", r.name, r.writers.join(", "));
        }
    }
    for (name, set) in &p.process_env {
        if *set {
            println!("{cyan}●{reset} %{name:<19} {dim}← 进程环境变量（已设置）{reset}");
        } else {
            println!("{yellow}⚠{reset} %{name:<19} {yellow}进程环境变量未设置{reset}");
        }
    }
    for u in &p.undefined {
        println!("{red}✘ {u} 未定义{reset}");
    }
    if p.undefined.is_empty() { 0 } else { 2 }
}

fn cmd_fmt(paths: &[String], check: bool) -> u8 {
    let paths: Vec<String> = if paths.is_empty() { vec!["collections".into()] } else { paths.to_vec() };
    let pb: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    let report = match fmt::format_paths(&pb, check) {
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
        }
    }
    if report.case_changes() == 0 {
        println!("\x1b[32m✔ 已是精简形态\x1b[0m");
    }
    if check && report.case_changes() > 0 { 1 } else { 0 }
}

fn cmd_doctor(json: bool) -> u8 {
    let r = doctor::run(std::path::Path::new("."));
    if json {
        println!("{}", serde_json::to_string_pretty(&r).unwrap_or_default());
    } else {
        println!("\x1b[1mrequestbaby doctor\x1b[0m");
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
    let paths: Vec<String> = if paths.is_empty() { vec!["collections".into()] } else { paths.to_vec() };
    let pb: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    let env = env
        .map(String::from)
        .or_else(|| default_env_of(&pb))
        .unwrap_or_else(|| "dev".into());
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
                std::fs::write(&file, serde_json::to_vec_pretty(&case).unwrap_or_default()).ok();
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
            println!("\n下一步: cd {} && requestbaby run collections --dry-run", sum.out_dir.display());
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
        watch_files.push(root.join("requestbaby.json"));
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
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("requestbaby"));
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
fn desktop_exe() -> Option<PathBuf> {
    let name = if cfg!(windows) { "requestbaby-desktop.exe" } else { "requestbaby-desktop" };
    if let Some(sibling) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(name))) {
        if sibling.is_file() {
            return Some(sibling);
        }
    }
    std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|d| d.join(name)).find(|p| p.is_file()))
}

fn cmd_open(path: Option<&str>) -> u8 {
    let dir = PathBuf::from(path.unwrap_or("."));
    let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
    let root = session::project_root(&dir).unwrap_or(dir);
    let Some(exe) = desktop_exe() else {
        eprintln!("\x1b[33m未找到 requestbaby-desktop。\x1b[0m 安装: cargo install --path crates/requestbaby-desktop");
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

fn cmd_history(limit: usize, json: bool) -> u8 {
    let Some(root) = session::project_root(std::path::Path::new(".")) else {
        eprintln!("\x1b[31m错误: 未找到项目根（requestbaby.json）\x1b[0m");
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
