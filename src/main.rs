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

pub const CASE_SCHEMA: &str = include_str!("../docs/schema/case.schema.json");

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
    /// 导出用例 JSON Schema（喂给 AI 约束生成）
    Schema,
    /// 校验配置：结构 + 变量引用 + 脚本语法
    Validate {
        /// 要校验的文件/目录
        #[arg(required = true)]
        paths: Vec<String>,
        /// 环境名（决定参与变量检查的 env 文件）
        #[arg(short, long)]
        env: Option<String>,
    },
    /// 列出可执行的用例
    List {
        paths: Vec<String>,
    },
    /// 导入外部格式：OpenAPI 3 / Swagger 2 / Postman Collection v2.1（JSON 或 YAML）
    Import {
        /// spec 文件路径
        spec: String,
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
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let code = match cli.cmd {
        Cmd::Init { path } => cmd_init(path.as_deref()),
        Cmd::Schema => {
            println!("{CASE_SCHEMA}");
            0
        }
        Cmd::Validate { paths, env } => cmd_validate(&paths, env.as_deref()),
        Cmd::List { paths } => cmd_list(&paths),
        Cmd::Import { spec, out, env } => cmd_import(&spec, out.as_deref(), env.as_deref()),
        Cmd::Send { case, env } => cmd_send(&case, env.as_deref()),
        Cmd::Watch { paths, env, tag, name } => cmd_watch(&paths, env.as_deref(), tag, name.as_deref()),
        Cmd::Run { paths, env, tag, name, fail_fast, dry_run, json, verbose, output, parallel, report } => {
            cmd_run(&paths, env.as_deref(), tag, name.as_deref(), fail_fast, dry_run, json, verbose, output.as_deref(), parallel, report.as_deref())
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

fn cmd_validate(paths: &[String], env: Option<&str>) -> u8 {
    let pb: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    let env = env
        .map(String::from)
        .or_else(|| default_env_of(&pb))
        .unwrap_or_else(|| "dev".into());
    match validate::validate_paths(&pb, &env) {
        Ok(issues) => {
            let errors = issues.iter().filter(|i| i.level == "error").count();
            let warns = issues.len() - errors;
            for i in &issues {
                let (color, sym) = if i.level == "error" { ("\x1b[31m", "✘") } else { ("\x1b[33m", "⚠") };
                println!("{color}{sym}{reset} {file}: {msg}", sym = sym, color = color, reset = "\x1b[0m", file = i.file, msg = i.message);
            }
            if issues.is_empty() {
                println!("\x1b[32m✔ 全部通过\x1b[0m");
                0
            } else {
                println!("\n{} 错误, {} 警告", errors, warns);
                if errors > 0 { 1 } else { 0 }
            }
        }
        Err(e) => {
            eprintln!("校验失败: {e:#}");
            1
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

        let mut r = runner::Runner::new(ctx);
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

fn cmd_import(spec: &str, out: Option<&str>, env: Option<&str>) -> u8 {
    let out_dir = std::path::PathBuf::from(out.unwrap_or("imported"));
    let env_name = env.unwrap_or("dev");
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
    let mut r = runner::Runner::new(ctx);
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

    let snapshot = || -> BTreeMapAll {
        let mut m = std::collections::BTreeMap::new();
        for f in &watch_files {
            if let Ok(meta) = std::fs::metadata(f) {
                m.insert(f.display().to_string(), (meta.len(), mtime_of(&meta)));
            }
        }
        m
    };
    let mut prev = snapshot();
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
        loop {
            std::thread::sleep(std::time::Duration::from_millis(600));
            let cur = snapshot();
            if cur != prev {
                prev = cur;
                break;
            }
        }
    }
}

type BTreeMapAll = std::collections::BTreeMap<String, (u64, std::time::SystemTime)>;

fn mtime_of(meta: &std::fs::Metadata) -> std::time::SystemTime {
    meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH)
}

/// 从第一个路径定位项目默认环境
fn default_env_of(paths: &[PathBuf]) -> Option<String> {
    let root = runner::discover::find_project_root(&paths[0])?;
    let p = runner::discover::project_file(&root).and_then(|pf| config::load_strict::<config::Project>(&pf).ok())?;
    Some(p.default_env)
}
