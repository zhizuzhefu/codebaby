mod config;
mod engine;
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
        Cmd::Run { paths, env, tag, name, fail_fast, dry_run, json, verbose } => {
            cmd_run(&paths, env.as_deref(), tag, name.as_deref(), fail_fast, dry_run, json, verbose)
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

        if json {
            println!("{}", report::render_json(&results, verbose));
            let failed = results.iter().filter(|x| !x.passed() && !x.skipped).count();
            return Ok(if failed > 0 { 1 } else { 0 });
        }
        // 收集后统一渲染，保证各列对齐（URL 完整显示，超长仅剥 query）
        report::print_results(&results, verbose || dry_run);
        report::print_summary(&results);
        let failed = results.iter().filter(|x| !x.passed() && !x.skipped).count();
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

/// 从第一个路径定位项目默认环境
fn default_env_of(paths: &[PathBuf]) -> Option<String> {
    let root = runner::discover::find_project_root(&paths[0])?;
    let p = runner::discover::project_file(&root).and_then(|pf| config::load_strict::<config::Project>(&pf).ok())?;
    Some(p.default_env)
}
