mod config;
mod engine;
mod importer;
mod report;
mod runner;
mod script;
mod validate;
mod vars;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;

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
    /// 环境自检（AI 调度前置）：curl 版本 / 配置完整性 / 引擎可用性
    Doctor,
    /// 导出 JSON Schema（喂给 AI 约束生成；--all 输出四类 bundle）
    Schema,
    /// 导出全部 Schema（case/collection/env/project）
    SchemaAll,
    /// 校验配置：Schema 结构 + 变量引用 + 脚本语法 + 引用存在性
    Validate {
        /// 要校验的文件/目录
        #[arg(required = true)]
        paths: Vec<String>,
        /// 环境名（决定参与变量检查的 env 文件）
        #[arg(short, long)]
        env: Option<String>,
        /// 结构化输出（AI 调度：file/level/rule/message）
        #[arg(long)]
        json: bool,
    },
    /// 列出可执行的用例
    List {
        paths: Vec<String>,
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
        /// 报告格式：term/json/junit/html
        #[arg(long)]
        report: Option<String>,
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
        /// 文件 / 目录 / glob
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
        /// JSON 报告输出（CI 友好）
        #[arg(long)]
        json: bool,
        /// 详细输出：curl 命令 / 全部断言 / 脚本日志
        #[arg(short, long)]
        verbose: bool,
        /// 响应落盘目录（每用例一个 JSON：meta + body）
        #[arg(short, long)]
        output: Option<String>,
        /// 独立用例并发档位（依赖捕获链/带脚本的仍串行在前；不支持 setup/teardown）
        #[arg(long)]
        parallel: Option<usize>,
        /// 报告格式：term（默认）/ json / junit / html（写 report.html）
        #[arg(long)]
        report: Option<String>,
        /// 执行引擎：curl（默认，调试保真）/ native（需 features=native 编译，连接复用）
        #[arg(long)]
        engine: Option<String>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let code = match cli.cmd {
        Cmd::Init { path } => cmd_init(path.as_deref()),
        Cmd::Doctor => cmd_doctor(),
        Cmd::Schema => {
            println!("{}", validate::CASE_SCHEMA);
            0
        }
        Cmd::SchemaAll => {
            println!(
                "{}",
                serde_json::json!({
                    "case": serde_json::from_str::<serde_json::Value>(validate::CASE_SCHEMA).unwrap(),
                    "collection": serde_json::from_str::<serde_json::Value>(validate::COLLECTION_SCHEMA).unwrap(),
                    "env": serde_json::from_str::<serde_json::Value>(validate::ENV_SCHEMA).unwrap(),
                    "project": serde_json::from_str::<serde_json::Value>(validate::PROJECT_SCHEMA).unwrap(),
                })
                .to_string()
            );
            0
        }
        Cmd::Validate { paths, env, json } => cmd_validate(&paths, env.as_deref(), json),
        Cmd::List { paths } => cmd_list(&paths),
        Cmd::Import { spec, curl, out, env } => cmd_import(spec.as_deref(), curl.as_deref(), out.as_deref(), env.as_deref()),
        Cmd::Send { case, env } => cmd_send(&case, env.as_deref()),
        Cmd::Workflow { name, env, dry_run, verbose, report } => {
            cmd_workflow(&name, env.as_deref(), dry_run, verbose, report.as_deref())
        }
        Cmd::Watch { paths, env, tag, name } => cmd_watch(&paths, env.as_deref(), tag, name.as_deref()),
        Cmd::Run { paths, env, tag, name, fail_fast, dry_run, json, verbose, output, parallel, report, engine } => {
            cmd_run(&paths, env.as_deref(), tag, name.as_deref(), fail_fast, dry_run, json, verbose, output.as_deref(), parallel, report.as_deref(), engine.as_deref())
        }
    };
    ExitCode::from(code)
}

fn cmd_init(path: Option<&str>) -> u8 {
    let dir = PathBuf::from(path.unwrap_or("."));
    let mk = |p: PathBuf, content: &str| -> Result<()> {
        if p.exists() {
            eprintln!("跳过已存在: {}", p.display());
            return Ok(());
        }
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&p, content)?;
        println!("创建: {}", p.display());
        Ok(())
    };
    let _ = mk(dir.join("requestbaby.json"), "{\n  \"version\": 1,\n  \"defaultEnv\": \"dev\",\n  \"vars\": {},\n  \"timeoutMs\": 30000\n}\n");
    let _ = mk(dir.join("envs/dev.json"), "{\n  \"vars\": {\n    \"baseUrl\": \"http://localhost:8080\"\n  }\n}\n");
    let _ = mk(dir.join("envs/prod.json"), "{\n  \"vars\": {\n    \"baseUrl\": \"https://api.example.com\"\n  }\n}\n");
    let _ = mk(dir.join("collections/demo/collection.json"), "{\n  \"name\": \"demo\",\n  \"tags\": [\"smoke\"],\n  \"vars\": {}\n}\n");
    let _ = mk(dir.join("collections/demo/01-health.json"), r#"{
  "name": "health",
  "tags": ["smoke"],
  "request": {
    "method": "GET",
    "url": "{{baseUrl}}/health"
  },
  "tests": [
    "res.status === 200",
    "res.json().status === 'ok'"
  ]
}
"#);
    let _ = mk(dir.join("collections/demo/02-login.json"), r#"{
  "name": "login",
  "tags": ["smoke", "auth"],
  "request": {
    "method": "POST",
    "url": "{{baseUrl}}/auth/login",
    "headers": { "Content-Type": "application/json" },
    "body": { "json": { "username": "admin", "password": "secret" } }
  },
  "post": "vars.token = res.json().token",
  "tests": [
    "res.status === 200",
    "vars.token.length > 0"
  ]
}
"#);
    println!("\n完成。试试: requestbaby run {}", dir.join("collections").display());
    0
}

fn cmd_validate(paths: &[String], env: Option<&str>, json: bool) -> u8 {
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
                println!("{color}{sym}{reset} {file} [{rule}]: {msg}", sym = sym, color = color, reset = "\x1b[0m", file = i.file, rule = i.rule, msg = i.message);
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

#[allow(clippy::too_many_arguments)]
fn cmd_run(
    paths: &[String],
    env: Option<&str>,
    tags: Vec<String>,
    name: Option<&str>,
    fail_fast: bool,
    dry_run: bool,
    json: bool,
    verbose: bool,
    output: Option<&str>,
    parallel: Option<usize>,
    report: Option<&str>,
    engine: Option<&str>,
) -> u8 {
    let pb: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    let result = (|| -> Result<u8> {
        let cases = runner::discover::discover(&pb)?;
        if cases.is_empty() {
            eprintln!("未发现用例（*.json，排除 requestbaby.json/collection.json/envs）");
            return Ok(1);
        }
        let env_name = env
            .map(String::from)
            .or_else(|| default_env_of(&pb))
            .unwrap_or_else(|| "dev".into());
        let ctx = runner::load_project_ctx(&pb[0], &env_name)?;
        let opts = runner::RunOptions {
            tags,
            name_filter: name.unwrap_or("").to_string(),
            fail_fast,
            dry_run,
            verbose,
            output_dir: output.map(std::path::PathBuf::from),
            send_mode: false,
            parallel: parallel,
        };

        if !json {
            println!(
                "\x1b[1mrequestbaby\x1b[0m \x1b[2menv={env_name} cases={} timeout={}ms{}\x1b[0m",
                cases.len(),
                ctx.project.timeout_ms,
                if dry_run { " [dry-run]" } else { "" }
            );
        }

        let mut r = runner::Runner::new(ctx, engine.unwrap_or("curl"));
        let results = r.run(&cases, &opts, |_| {})?;

        let fmt = report.unwrap_or(if json { "json" } else { "term" });
        let failed = results.iter().filter(|x| !x.passed() && !x.skipped).count();
        match fmt {
            "json" => println!("{}", report::render_json(&results, verbose)),
            "junit" => {
                let total_ms: u64 = results.iter().map(|r| r.duration_ms).sum();
                println!("{}", report::render_junit(&results, total_ms));
            }
            "html" => {
                let path = PathBuf::from("report.html");
                std::fs::write(&path, report::render_html(&results)).ok();
                if !json {
                    eprintln!("\x1b[2mHTML 报告: {}\x1b[0m", path.display());
                }
            }
            _ => {
                report::print_results(&results, verbose || dry_run);
                report::print_summary(&results);
            }
        }
        Ok(if failed > 0 { 1 } else { 0 })
    })();
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("\x1b[31m错误: {e:#}\x1b[0m");
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

fn cmd_send(case: &str, env: Option<&str>) -> u8 {
    let path = PathBuf::from(case);
    if !path.exists() {
        eprintln!("\x1b[31m错误: 用例不存在 {case}\x1b[0m");
        return 1;
    }
    let env_name = env
        .map(String::from)
        .or_else(|| default_env_of(std::slice::from_ref(&path)))
        .unwrap_or_else(|| "dev".into());
    let ctx = match runner::load_project_ctx(&path, &env_name) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("\x1b[31m错误: {e:#}\x1b[0m");
            return 1;
        }
    };
    let cases = match runner::discover::discover(std::slice::from_ref(&path)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("\x1b[31m错误: {e:#}\x1b[0m");
            return 1;
        }
    };
    let Some(dc) = cases.first() else {
        eprintln!("\x1b[31m错误: 未发现用例\x1b[0m");
        return 1;
    };
    let opts = runner::RunOptions {
        tags: vec![],
        name_filter: String::new(),
        fail_fast: false,
        dry_run: false,
        verbose: false,
        output_dir: None,
        send_mode: true,
        parallel: None,
    };
    let case_cfg = match config::load_strict::<config::TestCase>(&dc.path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("\x1b[31m配置错误: {e:#}\x1b[0m");
            return 1;
        }
    };
    let display_name = if case_cfg.name.is_empty() {
        dc.path.file_stem().unwrap_or_default().to_string_lossy().into_owned()
    } else {
        case_cfg.name.clone()
    };
    let mut r = runner::Runner::new(ctx, "curl");
    let results = match r.run(&cases, &opts, |_| {}) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("\x1b[31m错误: {e:#}\x1b[0m");
            return 1;
        }
    };
    let res = &results[0];

    // ---- 交互式展示 ----
    let cyan = "\x1b[36m";
    let green = "\x1b[32m";
    let yellow = "\x1b[33m";
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
    let _ = (yellow, display_name);
    if res.passed() { 0 } else { 1 }
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

fn cmd_watch(paths: &[String], env: Option<&str>, tags: Vec<String>, name: Option<&str>) -> u8 {
    let pb: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    if pb.is_empty() {
        eprintln!("错误: 缺少路径参数");
        return 1;
    }
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

fn cmd_workflow(name: &str, env: Option<&str>, dry_run: bool, verbose: bool, report: Option<&str>) -> u8 {
    let cwd = std::path::PathBuf::from(".");
    let Some(root) = runner::discover::find_project_root(&cwd) else {
        eprintln!("\x1b[31m错误: 未找到项目根（requestbaby.json）\x1b[0m");
        return 1;
    };
    let pf = match runner::discover::project_file(&root) {
        Some(p) => p,
        None => {
            eprintln!("\x1b[31m错误: 缺少 requestbaby.json\x1b[0m");
            return 1;
        }
    };
    let project = match config::load_strict::<config::Project>(&pf) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("\x1b[31m配置错误: {e:#}\x1b[0m");
            return 1;
        }
    };
    let Some(wf) = project.workflows.get(name) else {
        eprintln!(
            "\x1b[31m错误: 工作流 {name} 不存在。已定义: {}\x1b[0m",
            project.workflows.keys().cloned().collect::<Vec<_>>().join(", ")
        );
        return 1;
    };
    let env_name = env
        .map(String::from)
        .unwrap_or_else(|| project.default_env.clone());
    let ctx = match runner::load_project_ctx(&root, &env_name) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("\x1b[31m错误: {e:#}\x1b[0m");
            return 1;
        }
    };

    println!(
        "\x1b[1mworkflow\x1b[0m \x1b[1m{name}\x1b[0m \x1b[2menv={env_name} steps={}{}\x1b[0m",
        wf.steps.len(),
        if dry_run { " [dry-run]" } else { "" }
    );
    let mut r = runner::Runner::new(ctx, "curl");
    let mut all_results: Vec<runner::CaseResult> = vec![];
    let fmt = report.unwrap_or("term");
    let is_term = fmt == "term";

    for (i, step) in wf.steps.iter().enumerate() {
        let target = root.join(&step.run);
        let cases = match runner::discover::discover(std::slice::from_ref(&target)) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("\x1b[31m步骤 {} 发现用例失败: {e:#}\x1b[0m", step.run);
                return 1;
            }
        };
        if is_term {
            let label = if step.name.is_empty() { step.run.clone() } else { step.name.clone() };
            let mut extra = String::new();
            if !step.tags.is_empty() {
                extra.push_str(&format!(" --tag {}", step.tags.join(",")));
            }
            if !step.name_filter.is_empty() {
                extra.push_str(&format!(" --name {}", step.name_filter));
            }
            println!(
                "\x1b[2m── step {}/{}: {}{} ({})\x1b[0m",
                i + 1,
                wf.steps.len(),
                label,
                extra,
                cases.len()
            );
        }
        let opts = runner::RunOptions {
            tags: step.tags.clone(),
            name_filter: step.name_filter.clone(),
            fail_fast: true,
            dry_run,
            verbose,
            output_dir: None,
            send_mode: false,
            parallel: None,
        };
        let results = match r.run(&cases, &opts, |_| {}) {
            Ok(x) => x,
            Err(e) => {
                eprintln!("\x1b[31m步骤执行失败: {e:#}\x1b[0m");
                return 1;
            }
        };
        let step_failed = results.iter().any(|x| !x.passed() && !x.skipped);
        if is_term {
            report::print_results(&results, verbose || dry_run);
        }
        all_results.extend(results);
        if step_failed && !dry_run {
            if is_term {
                report::print_summary(&all_results);
                eprintln!("\x1b[31m工作流在步骤 {}/{} 失败中止\x1b[0m", i + 1, wf.steps.len());
            }
            return 1;
        }
    }

    emit_report(fmt, &all_results, verbose, false);
    let failed = all_results.iter().filter(|x| !x.passed() && !x.skipped).count();
    if failed > 0 { 1 } else { 0 }
}

/// 统一报告输出（run/workflow 共用）
fn emit_report(fmt: &str, results: &[runner::CaseResult], verbose: bool, quiet_note: bool) {
    match fmt {
        "json" => println!("{}", report::render_json(results, verbose)),
        "junit" => {
            let total_ms: u64 = results.iter().map(|r| r.duration_ms).sum();
            println!("{}", report::render_junit(results, total_ms));
        }
        "html" => {
            let path = PathBuf::from("report.html");
            std::fs::write(&path, report::render_html(results)).ok();
            if !quiet_note {
                eprintln!("\x1b[2mHTML 报告: {}\x1b[0m", path.display());
            }
            report::print_summary(results);
        }
        _ => {
            report::print_results(results, verbose);
            report::print_summary(results);
        }
    }
}

fn cmd_doctor() -> u8 {
    let mut ok = true;
    println!("\x1b[1mrequestbaby doctor\x1b[0m");
    // curl 引擎可用性
    match std::process::Command::new("curl").arg("--version").output() {
        Ok(o) if o.status.success() => {
            let v = String::from_utf8_lossy(&o.stdout);
            let first = v.lines().next().unwrap_or("curl");
            let ver = first.split_whitespace().nth(1).unwrap_or("?").to_string();
            let mut it = ver.split('.');
            let major = it.next().and_then(|m| m.parse::<u32>().ok()).unwrap_or(0);
            let minor = it.next().and_then(|m| m.parse::<u32>().ok()).unwrap_or(0);
            if major > 7 || (major == 7 && minor >= 75) {
                println!("  \x1b[32m✔\x1b[0m curl {ver}（支持 -w %{{json}}）");
            } else {
                println!("  \x1b[33m⚠\x1b[0m curl {ver} < 7.75，元数据采集可能失败");
            }
        }
        _ => {
            println!("  \x1b[31m✘\x1b[0m curl 未安装（引擎不可用）");
            ok = false;
        }
    }
    // native 引擎
    #[cfg(feature = "native")]
    println!("  \x1b[32m✔\x1b[0m native 引擎已编译（--engine native）");
    #[cfg(not(feature = "native"))]
    println!("  \x1b[2m-\x1b[0m native 引擎未编译（可选：cargo build --features native）");
    // 项目上下文
    let cwd = std::path::Path::new(".");
    match runner::discover::find_project_root(cwd) {
        Some(root) => {
            println!("  \x1b[32m✔\x1b[0m 项目根: {}", root.display());
            if let Some(pf) = runner::discover::project_file(&root) {
                match config::load_strict::<config::Project>(&pf) {
                    Ok(p) => println!("  \x1b[32m✔\x1b[0m 项目配置 ok（defaultEnv={} workflows={}）", p.default_env, p.workflows.len()),
                    Err(e) => {
                        println!("  \x1b[31m✘\x1b[0m 项目配置解析失败: {e:#}");
                        ok = false;
                    }
                }
                let envs_dir = root.join("envs");
                if envs_dir.is_dir() {
                    let n = std::fs::read_dir(&envs_dir).map(|d| d.filter_map(|e| e.ok()).count()).unwrap_or(0);
                    println!("  \x1b[32m✔\x1b[0m 环境文件 {n} 个");
                } else {
                    println!("  \x1b[33m⚠\x1b[0m 无 envs/ 目录");
                }
            }
        }
        None => println!("  \x1b[33m⚠\x1b[0m 当前不在项目内（无 requestbaby.json）"),
    }
    if ok { 0 } else { 2 }
}

/// 从第一个路径定位项目默认环境
fn default_env_of(paths: &[PathBuf]) -> Option<String> {
    let root = runner::discover::find_project_root(&paths[0])?;
    let p = runner::discover::project_file(&root).and_then(|pf| config::load_strict::<config::Project>(&pf).ok())?;
    Some(p.default_env)
}
