//! 项目内容的增删改命令：初始化、新建、流程 / 环境 / 变量管理

use super::*;

pub(crate) fn cmd_init(path: Option<&str>, base_url: Option<&str>) -> u8 {
    let dir = PathBuf::from(path.unwrap_or("."));
    match scaffold::init_project_with(&dir, base_url) {
        Ok(entries) => {
            for e in entries {
                if e.created {
                    println!("创建: {}", e.path.display());
                } else {
                    eprintln!("跳过已存在: {}", e.path.display());
                }
            }
            let cd = path.filter(|p| *p != ".").map(|p| format!("cd {p} && ")).unwrap_or_default();
            println!("\n完成。下一步：");
            if base_url.is_none() {
                println!("  1. 告诉它被测服务在哪（示例用例会请求 http://localhost:8080，没有这个服务时先改成你的）：");
                println!("       {cd}codebaby vars set baseUrl http://主机:端口 -e dev");
                println!("  2. 跑一遍：codebaby run");
            } else {
                println!("  跑一遍（被测服务要先启动）：{cd}codebaby run");
            }
            println!("  新建自己的用例：codebaby new case collections/<目录> <名字>；写法速查：codebaby guide");
            0
        }
        Err(e) => err(format!("{e:#}")),
    }
}

pub(crate) fn cmd_guide_init(update: bool) -> u8 {
    let root = match project_root_or_err() {
        Ok(r) => r,
        Err(code) => return code,
    };
    let status = guide::agent_entry_status(&root);
    match guide::write_agent_entry_with(&root, update) {
        Ok(entries) => {
            for e in entries {
                let agents = e.path.file_name().is_some_and(|n| n == "AGENTS.md");
                if e.created && agents && status == guide::AgentEntryStatus::Outdated {
                    println!("已更新: {}", e.path.display());
                } else if e.created {
                    println!("创建: {}", e.path.display());
                } else if agents && status == guide::AgentEntryStatus::Outdated {
                    eprintln!("跳过已存在: {}（是旧版入口：加 --update 更新）", e.path.display());
                } else {
                    eprintln!("跳过已存在: {}", e.path.display());
                }
            }
            0
        }
        Err(e) => err(format!("{e:#}")),
    }
}

pub(crate) fn cmd_new(what: NewCmd) -> u8 {
    let created = match what {
        NewCmd::Case(NewCaseArgs { dir, name, curl: Some(curl), env, .. }) => new_case_from_curl(&dir, name.as_deref(), &curl, env.as_deref()),
        NewCmd::Case(NewCaseArgs { name: None, .. }) => Err(anyhow::anyhow!("缺少用例名（或用 --curl 从 curl 命令生成）")),
        NewCmd::Case(NewCaseArgs { dir, name: Some(name), grpc: Some(method), target, env, .. }) => new_grpc_case(&dir, &name, &method, target.as_deref(), env.as_deref()),
        NewCmd::Case(NewCaseArgs { dir, name: Some(name), ws: Some(url), .. }) => scaffold::new_ws_case(&PathBuf::from(dir), &name, &url),
        NewCmd::Case(NewCaseArgs { dir, name: Some(name), web: Some(url), .. }) => scaffold::new_web_case(&PathBuf::from(dir), &name, &url),
        NewCmd::Case(NewCaseArgs { dir, name: Some(name), android: Some(app), .. }) => scaffold::new_android_case(&PathBuf::from(dir), &name, &app),
        NewCmd::Case(NewCaseArgs { dir, name: Some(name), desktop: Some(app), .. }) => scaffold::new_desktop_case(&PathBuf::from(dir), &name, &app),
        NewCmd::Case(NewCaseArgs { dir, name: Some(name), method, url, .. }) => scaffold::new_case(&PathBuf::from(dir), &name, &method, &url),
        NewCmd::Collection(NewCollectionArgs { parent, name }) => scaffold::new_collection(&PathBuf::from(parent), &name),
        NewCmd::Env(NewEnvArgs { name, from }) => match session::project_root(std::path::Path::new(".")) {
            Some(root) => envs::create(&root, &name, from.as_deref()),
            None => Err(anyhow::anyhow!("未找到项目根（codebaby.json）")),
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

pub(crate) fn cmd_workflow_manage(action: WorkflowCmd) -> u8 {
    let root = match project_root_or_err() {
        Ok(r) => r,
        Err(_) => return 2,
    };
    let done = |msg: String| {
        println!("\x1b[32m✔\x1b[0m {msg}");
        0
    };
    let pos = |n: usize| n.checked_sub(1).ok_or_else(|| anyhow::anyhow!("位置从 1 开始"));
    let r: anyhow::Result<u8> = (|| match action {
        WorkflowCmd::List { json } => {
            let all = workflow::list(&root)?;
            if json {
                let v: serde_json::Map<String, serde_json::Value> =
                    all.iter().map(|(k, w)| (k.clone(), serde_json::to_value(w).unwrap_or_default())).collect();
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else if all.is_empty() {
                println!("还没有流程：codebaby workflow new <key> --step <目录或用例>");
            } else {
                for (k, w) in &all {
                    let name = if w.name.is_empty() { String::new() } else { format!(" \x1b[2m{}\x1b[0m", w.name) };
                    let env = w.env.as_ref().map(|e| format!(" · 环境 {e}")).unwrap_or_default();
                    let needs = if w.needs.is_empty() { String::new() } else { format!(" · 依赖 {}", w.needs.join("、")) };
                    println!("\x1b[1m{k}\x1b[0m{name} \x1b[2m· {} 步{env}{needs}\x1b[0m", w.steps.len());
                }
            }
            Ok(0)
        }
        WorkflowCmd::Show { key, json } => {
            let w = workflow::get(&root, &key)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&w)?);
                return Ok(0);
            }
            println!("\x1b[1m{key}\x1b[0m{}", if w.name.is_empty() { String::new() } else { format!(" · {}", w.name) });
            if let Some(e) = &w.env {
                println!("  \x1b[2m固定环境 {e}\x1b[0m");
            }
            if !w.needs.is_empty() {
                println!("  \x1b[2m依赖 {}（先运行，登录态沿用到下面所有步骤）\x1b[0m", w.needs.join(" → "));
            }
            if w.steps.is_empty() && w.needs.is_empty() {
                println!("  （没有步骤：codebaby workflow step add {key} <目录或用例>）");
            }
            for (i, st) in w.steps.iter().enumerate() {
                let mut extra = String::new();
                if !st.tags.is_empty() {
                    extra.push_str(&format!(" --tag {}", st.tags.join(",")));
                }
                if !st.name_filter.is_empty() {
                    extra.push_str(&format!(" --name {}", st.name_filter));
                }
                if let Some(u) = &st.as_user {
                    extra.push_str(&format!(" · 用户 {u}"));
                }
                let label = if st.name.is_empty() { String::new() } else { format!("{}  ", st.name) };
                println!("  {}. {label}\x1b[2m{}{extra}\x1b[0m", i + 1, st.run);
            }
            Ok(0)
        }
        WorkflowCmd::New { key, name, steps, env, needs } => {
            let wf = config::Workflow {
                name: name.unwrap_or_default(),
                env,
                needs,
                steps: steps.into_iter().map(|run| config::WorkflowStep { run, ..Default::default() }).collect(),
            };
            workflow::save(&root, None, &key, &wf)?;
            let needs = if wf.needs.is_empty() { String::new() } else { format!("，依赖 {}", wf.needs.join("、")) };
            Ok(done(format!("已新建流程 {key}（{} 步{needs}）", wf.steps.len())))
        }
        WorkflowCmd::Needs { key, action: None } => {
            let w = workflow::get(&root, &key)?;
            if w.needs.is_empty() {
                println!("流程 {key} 没有依赖（codebaby workflow needs {key} add <流程>）");
            } else {
                println!("流程 {key} 依赖（按顺序先运行）：{}", w.needs.join(" → "));
            }
            Ok(0)
        }
        WorkflowCmd::Needs { key, action: Some(NeedsCmd::Add { need }) } => {
            let needs = workflow::add_need(&root, &key, &need)?;
            Ok(done(format!("流程 {key} 依赖：{}", needs.join("、"))))
        }
        WorkflowCmd::Needs { key, action: Some(NeedsCmd::Rm { need }) } => {
            let needs = workflow::remove_need(&root, &key, &need)?;
            Ok(done(if needs.is_empty() { format!("流程 {key} 不再有依赖") } else { format!("流程 {key} 依赖：{}", needs.join("、")) }))
        }
        WorkflowCmd::Rename { key, new_key, name } => {
            let mut wf = workflow::get(&root, &key)?;
            if let Some(n) = name {
                wf.name = n;
            }
            let new_key = new_key.unwrap_or_else(|| key.clone());
            workflow::save(&root, Some(&key), &new_key, &wf)?;
            Ok(done(if new_key == key { format!("已更新流程 {key}") } else { format!("已重命名 {key} → {new_key}") }))
        }
        WorkflowCmd::Env { key, env, unset } => {
            let mut wf = workflow::get(&root, &key)?;
            wf.env = if unset { None } else { env };
            workflow::save(&root, Some(&key), &key, &wf)?;
            Ok(done(match &wf.env {
                Some(e) => format!("流程 {key} 固定在环境 {e}（verify 会在 {e} 运行它）"),
                None => format!("流程 {key} 跟随 -e / defaultEnv 运行"),
            }))
        }
        WorkflowCmd::Cp { key, new_key } => {
            workflow::duplicate(&root, &key, &new_key)?;
            Ok(done(format!("已复制 {key} → {new_key}")))
        }
        WorkflowCmd::Rm { key } => {
            workflow::delete(&root, &key)?;
            Ok(done(format!("已删除流程 {key}")))
        }
        WorkflowCmd::Step { action: StepCmd::Add { key, run, name, tag, name_filter, at, as_user } } => {
            let step = config::WorkflowStep {
                name: name.unwrap_or_default(),
                run: run.clone(),
                tags: tag.iter().flat_map(|t| t.split(',')).map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect(),
                name_filter: name_filter.unwrap_or_default(),
                as_user: as_user.filter(|u| !u.trim().is_empty()),
            };
            let at = at.map(pos).transpose()?;
            let ix = workflow::add_step(&root, &key, step, at)?;
            Ok(done(format!("流程 {key}：第 {} 步 {run}", ix + 1)))
        }
        WorkflowCmd::Step { action: StepCmd::Rm { key, index } } => {
            let st = workflow::remove_step(&root, &key, pos(index)?)?;
            Ok(done(format!("流程 {key}：已删除第 {index} 步 {}", st.run)))
        }
        WorkflowCmd::Step { action: StepCmd::Mv { key, from, to } } => {
            workflow::move_step(&root, &key, pos(from)?, pos(to)?)?;
            Ok(done(format!("流程 {key}：第 {from} 步 → 第 {to} 位")))
        }
    })();
    r.unwrap_or_else(|e| fatal(format!("{e:#}")))
}

pub(crate) fn cmd_env(action: EnvCmd) -> u8 {
    let root = match project_root_or_err() {
        Ok(r) => r,
        Err(_) => return 2,
    };
    let done = |msg: String| {
        println!("\x1b[32m✔\x1b[0m {msg}");
        0
    };
    let r: anyhow::Result<u8> = (|| match action {
        EnvCmd::List { json } => {
            let all = envs::list(&root)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&all)?);
            } else if all.is_empty() {
                println!("还没有环境：codebaby env new <名称>");
            }
            for e in all.iter().filter(|_| !json) {
                let mut meta = vec![format!("{} 个变量", e.vars)];
                if let Some(n) = e.local_vars {
                    meta.push(format!("本机覆盖 {n}"));
                }
                if !e.pinned_by.is_empty() {
                    meta.push(format!("固定流程 {}", e.pinned_by.join("、")));
                }
                if let Some(err) = &e.error {
                    meta.push(format!("\x1b[31m{err}\x1b[0m\x1b[2m"));
                }
                let default = if e.is_default { " \x1b[32m默认\x1b[0m" } else { "" };
                let base = e.base_url.as_ref().map(|b| format!("  {b}")).unwrap_or_default();
                println!("\x1b[1m{}\x1b[0m{default}{base} \x1b[2m· {}\x1b[0m", e.name, meta.join(" · "));
            }
            Ok(0)
        }
        EnvCmd::Show { name, json } => {
            let vars = varstore::read(&root, &VarScope::Env(name.clone()))?;
            if !config::env_path(&root, &name).exists() {
                anyhow::bail!("环境不存在: {name}");
            }
            if json {
                let m: serde_json::Map<String, serde_json::Value> = vars.iter().map(|(k, d)| (k.clone(), d.encode())).collect();
                println!("{}", serde_json::to_string_pretty(&m)?);
                return Ok(0);
            }
            println!("\x1b[1m{name}\x1b[0m \x1b[2m{}\x1b[0m", config::env_path(&root, &name).strip_prefix(&root).unwrap_or(&root).display());
            if vars.is_empty() {
                println!("  （没有变量：codebaby vars set <名称> <值> -e {name}）");
            }
            for (k, d) in &vars {
                println!("  {k} = {}", d.display(k));
            }
            Ok(0)
        }
        EnvCmd::New(NewEnvArgs { name, from }) => {
            envs::create(&root, &name, from.as_deref())?;
            Ok(done(format!("已新建环境 {name}{}", from.map(|f| format!("（复制自 {f}）")).unwrap_or_default())))
        }
        EnvCmd::Rename { name, new_name } => {
            envs::rename(&root, &name, &new_name)?;
            Ok(done(format!("已重命名环境 {name} → {new_name}（本机覆盖、defaultEnv、固定的流程已同步）")))
        }
        EnvCmd::Cp { name, new_name } => {
            envs::duplicate(&root, &name, &new_name)?;
            Ok(done(format!("已复制环境 {name} → {new_name}（本机覆盖不复制）")))
        }
        EnvCmd::Rm { name } => {
            let how = envs::delete(&root, &name)?;
            Ok(done(format!("已删除环境 {name}（{}）", deleted_word(how))))
        }
        EnvCmd::Default { name } => {
            envs::set_default(&root, &name)?;
            Ok(done(format!("默认环境：{name}")))
        }
    })();
    r.unwrap_or_else(|e| fatal(format!("{e:#}")))
}

/// 用例 / 集合的重命名、复制、删除（new 走 cmd_new）
pub(crate) enum ItemOp {
    Rename { path: String, name: String, dir: bool },
    Cp { path: String, name: String, dir: bool },
    Rm { path: String, dir: bool },
}

impl From<CaseCmd> for ItemOp {
    fn from(c: CaseCmd) -> Self {
        match c {
            CaseCmd::Rename { path, name } => ItemOp::Rename { path, name, dir: false },
            CaseCmd::Cp { path, name } => ItemOp::Cp { path, name, dir: false },
            CaseCmd::Rm { path } => ItemOp::Rm { path, dir: false },
            CaseCmd::New(_) => unreachable!("case new 由 cmd_new 处理"),
        }
    }
}

impl From<CollectionCmd> for ItemOp {
    fn from(c: CollectionCmd) -> Self {
        match c {
            CollectionCmd::Rename { dir, name } => ItemOp::Rename { path: dir, name, dir: true },
            CollectionCmd::Cp { dir, name } => ItemOp::Cp { path: dir, name, dir: true },
            CollectionCmd::Rm { dir } => ItemOp::Rm { path: dir, dir: true },
            CollectionCmd::New(_) => unreachable!("collection new 由 cmd_new 处理"),
        }
    }
}

pub(crate) fn cmd_item(op: ItemOp) -> u8 {
    let (path, want_dir) = match &op {
        ItemOp::Rename { path, dir, .. } | ItemOp::Cp { path, dir, .. } | ItemOp::Rm { path, dir } => (PathBuf::from(path), *dir),
    };
    let what = if want_dir { "集合" } else { "用例" };
    if want_dir && !path.is_dir() {
        return fatal(format!("集合目录不存在: {}", path.display()));
    }
    if !want_dir && !(path.is_file() && path.extension().is_some_and(|e| e == "json")) {
        return fatal(format!("用例文件不存在: {}", path.display()));
    }
    let r = match op {
        ItemOp::Rename { name, .. } => scaffold::rename(&path, &name).map(|p| format!("已重命名{what} → {}", p.display())),
        ItemOp::Cp { name, .. } => scaffold::duplicate(&path, &name).map(|p| format!("已复制{what} → {}", p.display())),
        ItemOp::Rm { .. } => scaffold::delete(&path).map(|how| format!("已删除{what} {}（{}）", path.display(), deleted_word(how))),
    };
    match r {
        Ok(msg) => {
            println!("\x1b[32m✔\x1b[0m {msg}");
            0
        }
        Err(e) => fatal(format!("{e:#}")),
    }
}

pub(crate) fn deleted_word(how: scaffold::Deleted) -> &'static str {
    match how {
        scaffold::Deleted::Trashed => "已移到废纸篓，可还原",
        scaffold::Deleted::Removed => "系统不支持废纸篓，已永久删除",
    }
}

pub(crate) fn cmd_vars(case: &str, env: Option<&str>, json: bool) -> u8 {
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
    if let Ok(c) = codebaby_core::config::load_strict::<codebaby_core::config::TestCase>(&path) {
        let (method, url) = c.summary();
        println!("{dim}请求{reset} {method} {}", p.render_preview(&url));
    }
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

pub(crate) fn project_root_or_err() -> Result<PathBuf, u8> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    session::project_root(&cwd).ok_or_else(|| err("未找到项目根（codebaby.json）"))
}

pub(crate) fn cmd_vars_matrix(json: bool) -> u8 {
    let root = match project_root_or_err() {
        Ok(r) => r,
        Err(code) => return code,
    };
    let m = match varstore::matrix(&root) {
        Ok(m) => m,
        Err(e) => return err(format!("{e:#}")),
    };
    let undefined = m.undefined().count();
    if json {
        println!("{}", serde_json::to_string_pretty(&m).unwrap_or_default());
        return if undefined == 0 { 0 } else { 2 };
    }
    let (dim, red, cyan, reset) = ("\x1b[2m", "\x1b[31m", "\x1b[36m", "\x1b[0m");
    let cell = |s: &str| {
        let s: String = s.chars().take(22).collect();
        format!("{s:<24}")
    };
    println!("\x1b[1m变量总览\x1b[0m {dim}优先级：低 → 高（右侧覆盖左侧），运行时捕获最高{reset}");
    print!("{:<20}", "");
    for c in &m.columns {
        print!("{cyan}{}{reset}", cell(&c.label));
    }
    println!("{dim}引用{reset}");
    for r in &m.rows {
        print!("{:<20}", r.name);
        for c in &r.cells {
            match c {
                Some(d) => print!("{}", cell(&d.display(&r.name))),
                None => print!("{dim}{}{reset}", cell("·")),
            }
        }
        let mut note = format!("{} 个用例", r.used_by.len());
        if !r.runtime_writers.is_empty() {
            note.push_str(&format!(" · 运行时捕获 ← {}", r.runtime_writers.join(", ")));
        }
        if r.cells.iter().all(Option::is_none) && r.runtime_writers.is_empty() {
            println!("{red}{note} · 未定义{reset}");
        } else {
            println!("{dim}{note}{reset}");
        }
    }
    if undefined == 0 { 0 } else { 2 }
}

pub(crate) fn cmd_vars_edit(cmd: VarsCmd) -> u8 {
    let root = match project_root_or_err() {
        Ok(r) => r,
        Err(code) => return code,
    };
    let rel = |p: &std::path::Path| p.strip_prefix(&root).unwrap_or(p).display().to_string();
    match cmd {
        VarsCmd::Set { name, value, scope, secret, from_env } => {
            let scope = scope.scope(&root);
            let def = match (from_env, value) {
                (Some(n), _) => VarDef::FromEnv((!n.is_empty()).then_some(n)),
                (None, Some(v)) if secret => VarDef::Secret(v),
                (None, Some(v)) => VarDef::Plain(v),
                (None, None) => return err("缺少值（或使用 --from-env）"),
            };
            match varstore::set(&root, &scope, &name, &def) {
                Ok(file) => {
                    println!("\x1b[32m✔\x1b[0m {name} = {} \x1b[2m← {} · {}\x1b[0m", def.display(&name), scope.label(), rel(&file));
                    0
                }
                Err(e) => err(format!("{e:#}")),
            }
        }
        VarsCmd::Unset { name, scope } => {
            let scope = scope.scope(&root);
            match varstore::unset(&root, &scope, &name) {
                Ok(true) => {
                    println!("\x1b[32m✔\x1b[0m 已从{}删除 {name}", scope.label());
                    0
                }
                Ok(false) => err(format!("{}中没有变量 {name}", scope.label())),
                Err(e) => err(format!("{e:#}")),
            }
        }
    }
}

pub(crate) fn new_case_from_curl(dir: &str, name: Option<&str>, curl: &str, env: Option<&str>) -> anyhow::Result<PathBuf> {
    let curl = if curl == "-" {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
        buf
    } else {
        curl.to_string()
    };
    let dir = PathBuf::from(dir);
    let base_url = session::project_root(&dir).and_then(|root| {
        let env = session::resolve_env(std::slice::from_ref(&root), env);
        config::load_env(&root, &env).ok().flatten()?.vars.get("baseUrl").map(|v| v.value.clone())
    });
    let cwd = std::env::current_dir().ok();
    scaffold::new_case_from_curl(&dir, name, &curl, base_url.as_deref(), cwd.as_deref())
}
