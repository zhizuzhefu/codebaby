use crate::config::{load_strict, Collection, EnvFile, Project, TestCase};
use crate::runner::discover::{self, DiscoveredCase};
use crate::script::{QuickJsHost, ScriptHost};
use crate::vars::{find_undefined, VarVal, Vars};
use anyhow::Result;
use std::path::Path;

/// 校验问题（AI 可消费：file/level/rule/message 四元组）
#[derive(serde::Serialize, Clone, Debug)]
pub struct ValidationIssue {
    pub file: String,
    pub level: &'static str, // error / warn
    pub rule: &'static str,
    pub message: String,
    /// `requestbaby fmt --fix` 能自动改写（写法建议类问题）
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub fixable: bool,
}

impl ValidationIssue {
    /// 写法建议（不影响验收结论）：`fmt --fix` 能改的一键改写，其余交给 AI；人不需要逐条处理
    pub fn is_style(&self) -> bool {
        self.level == "warn" && crate::teach::STYLE_RULES.contains(&self.rule)
    }
}

/// 给 AI 的修改指令：按文件列出每条建议与改法（粘贴到对话即可）；没有可交给 AI 的建议时为空。env 为空则命令里不带 -e
pub fn ai_brief(issues: &[ValidationIssue], env: &str) -> String {
    let todo: Vec<&ValidationIssue> = issues.iter().filter(|i| i.is_style() && !i.fixable).collect();
    if todo.is_empty() {
        return String::new();
    }
    let mut files: Vec<&str> = todo.iter().map(|i| i.file.as_str()).collect();
    files.dedup();
    let mut out = String::from("请修改 requestbaby 项目里下面这些用例的写法（路径相对项目根；只改写法，不改变验收的内容）。先读 `requestbaby guide` 了解写法。\n");
    for f in files {
        out.push_str(&format!("\n{f}\n"));
        for i in todo.iter().filter(|i| i.file == f) {
            out.push_str(&format!("- [{}] {}\n", i.rule, i.message));
        }
    }
    let e = if env.is_empty() { String::new() } else { format!(" -e {env}") };
    out.push_str(&format!("\n改完执行 `requestbaby validate{e}` 确认这些建议消失，再 `requestbaby run{e}` 确认结论没变。\n"));
    out
}

pub const CASE_SCHEMA: &str = include_str!("../../../docs/schema/case.schema.json");
pub const COLLECTION_SCHEMA: &str = include_str!("../../../docs/schema/collection.schema.json");
pub const ENV_SCHEMA: &str = include_str!("../../../docs/schema/env.schema.json");
pub const PROJECT_SCHEMA: &str = include_str!("../../../docs/schema/project.schema.json");

fn schema_check(schema_json: &str, doc_path: &Path, doc_name: &str, issues: &mut Vec<ValidationIssue>) -> bool {
    let raw = match std::fs::read_to_string(doc_path) {
        Ok(r) => r,
        Err(_) => return false,
    };
    let doc: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            issues.push(ValidationIssue {
                fixable: false,
                file: doc_path.display().to_string(),
                level: "error",
                rule: "json-parse",
                message: format!("JSON 解析失败: {e}"),
            });
            return false;
        }
    };
    let schema: serde_json::Value = serde_json::from_str(schema_json).expect("内置 schema 合法");
    let compiled = jsonschema::JSONSchema::compile(&schema).expect("内置 schema 可编译");
    if let Err(errors) = compiled.validate(&doc) {
        for e in errors {
            issues.push(ValidationIssue {
                fixable: false,
                file: doc_path.display().to_string(),
                level: "error",
                rule: "schema",
                message: format!("[{doc_name}] {}", e),
            });
        }
        return false;
    }
    true
}

/// 校验一套用例目录：
/// 1. 四类配置文件 JSON Schema 结构校验（内置 schema，防漂移）
/// 2. serde 严格解析（deny_unknown_fields 兜底）
/// 3. {{var}} 引用存在性（含脚本写入、跨用例传递）
/// 4. pre/post/tests 脚本语法预编译
/// 5. workflow step.run 路径与 cases 数据文件存在性
pub fn validate_paths(paths: &[std::path::PathBuf], env_name: &str) -> Result<Vec<ValidationIssue>> {
    let mut issues = vec![];
    let host = QuickJsHost::new();

    let roots: Vec<_> = paths.iter().map(|p| discover::find_project_root(p)).collect();
    let root0 = roots.iter().flatten().next().cloned();
    let mut base_vars = Vars::new();
    if let Some(root) = &root0 {
        if let Some(pf) = discover::project_file(root) {
            if schema_check(PROJECT_SCHEMA, &pf, "project", &mut issues) {
                if let Ok(p) = load_strict::<Project>(&pf) {
                    base_vars.push_str_layer(p.vars.plain());
                    if let Some(px) = &p.proxy {
                        let problem = match (px.enabled, px.url.trim()) {
                            (true, "") => Some("proxy 已启用但没写 url：写 socks5://主机:端口，或把 enabled 设为 false".to_string()),
                            (_, url) if !url.is_empty() => crate::proxy_setting::check_url(url).err().map(|e| format!("proxy.url：{e:#}")),
                            _ => None,
                        };
                        if let Some(message) = problem {
                            issues.push(ValidationIssue { fixable: false, file: pf.display().to_string(), level: "error", rule: "proxy-url", message });
                        }
                    }
                    // workflow step.run 存在性
                    for (wf_name, wf) in &p.workflows {
                        if wf.steps.is_empty() {
                            issues.push(ValidationIssue {
                                fixable: false,
                                file: pf.display().to_string(),
                                level: "warn",
                                rule: "workflow-empty",
                                message: format!("流程 {wf_name} 没有步骤，运行会直接报错"),
                            });
                        }
                        if let Some(env) = &wf.env {
                            if !crate::config::env_path(root, env).exists() {
                                issues.push(ValidationIssue {
                                    fixable: false,
                                    file: pf.display().to_string(),
                                    level: "error",
                                    rule: "workflow-env",
                                    message: format!("workflows.{wf_name}.env 指定的环境不存在: envs/{env}.json"),
                                });
                            }
                        }
                        for (i, step) in wf.steps.iter().enumerate() {
                            let target = root.join(&step.run);
                            if !target.exists() {
                                issues.push(ValidationIssue {
                                    fixable: false,
                                    file: pf.display().to_string(),
                                    level: "error",
                                    rule: "workflow-path",
                                    message: format!(
                                        "workflows.{wf_name}.steps[{i}].run 不存在: {}",
                                        step.run
                                    ),
                                });
                            }
                        }
                    }
                }
            }
        }
        let env_path = root.join("envs").join(format!("{env_name}.json"));
        // 项目在用 envs/ 却找不到所选环境：多半是环境名写错，静默当空环境只会换来一堆莫名的失败
        if !env_path.exists() && root.join("envs").is_dir() {
            issues.push(ValidationIssue {
                fixable: false,
                file: format!("envs/{env_name}.json"),
                level: "error",
                rule: "env-missing",
                message: format!("环境 {env_name} 不存在（新建：requestbaby new env {env_name} --from <已有环境>）"),
            });
        }
        // 环境文件 + 本机覆盖（envs/<env>.local.json）都校验结构，变量都算已定义
        for path in [env_path, crate::config::local_env_path(root, env_name)] {
            if path.exists() && schema_check(ENV_SCHEMA, &path, "env", &mut issues) {
                if let Ok(f) = load_strict::<EnvFile>(&path) {
                    base_vars.push_str_layer(f.vars.plain());
                }
            }
        }
    }

    // 写法建议（与 fmt --fix 同一套规则）的项目上下文：各环境 baseUrl、固定环境的流程
    let ctx = root0.as_deref().map(crate::teach::Context::load).unwrap_or_default();

    // 各目录层级 collection.json
    for p in paths {
        check_collections_under(p, &ctx, &mut issues);
    }

    // 用例
    let cases = discover::discover(paths)?;
    let mut global_written: Vec<String> = vec![];
    for dc in &cases {
        let mut writes = load_strict::<TestCase>(&dc.path).map(|c| c.var_writes()).unwrap_or_default();
        // 集合 setup 写入的变量（造数）：同样是运行时定义
        writes.extend(dc.chain.iter().flat_map(|c| scan_var_writes(&c.setup)));
        for n in writes {
            if !global_written.contains(&n) {
                global_written.push(n);
            }
        }
    }
    for dc in &cases {
        validate_case(dc, &base_vars, &global_written, &host, &ctx, &mut issues);
    }
    Ok(issues)
}

/// 写法建议 → warn 问题（规则名即 guide 主题线索；fixable 的可由 fmt --fix 改写）
fn push_advice(file: &str, advice: Vec<crate::teach::Advice>, issues: &mut Vec<ValidationIssue>) {
    for a in advice {
        issues.push(ValidationIssue { file: file.to_string(), level: "warn", rule: a.rule, message: a.message, fixable: a.fixable });
    }
}

fn check_collections_under(p: &Path, ctx: &crate::teach::Context, issues: &mut Vec<ValidationIssue>) {
    let dir = if p.is_dir() { p.to_path_buf() } else { return };
    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name() == "collection.json")
    {
        if schema_check(COLLECTION_SCHEMA, entry.path(), "collection", issues) {
            if let Some(doc) = std::fs::read_to_string(entry.path()).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) {
                let dir = entry.path().parent().unwrap_or(Path::new("."));
                push_advice(&entry.path().display().to_string(), crate::teach::upgrade_collection(&doc, dir, ctx).1, issues);
            }
            if let Err(e) = load_strict::<Collection>(entry.path()) {
                issues.push(ValidationIssue {
                    fixable: false,
                    file: entry.path().display().to_string(),
                    level: "error",
                    rule: "serde-parse",
                    message: format!("{e:#}"),
                });
            }
        }
    }
}

fn validate_case(
    dc: &DiscoveredCase,
    base: &Vars,
    global_written: &[String],
    host: &QuickJsHost,
    ctx: &crate::teach::Context,
    issues: &mut Vec<ValidationIssue>,
) {
    let file = dc.rel.clone();
    if !schema_check(CASE_SCHEMA, &dc.path, "case", issues) {
        return;
    }
    let case = match load_strict::<TestCase>(&dc.path) {
        Ok(c) => c,
        Err(e) => {
            issues.push(ValidationIssue {
                fixable: false,
                file,
                level: "error",
                rule: "serde-parse",
                message: format!("{e:#}"),
            });
            return;
        }
    };

    // cases 数据文件存在性
    if let Some(crate::config::CasesSpec::File { file: data_file }) = &case.cases {
        let p = dc.path.parent().unwrap_or(Path::new(".")).join(data_file);
        if !p.exists() {
            issues.push(ValidationIssue {
                fixable: false,
                file: file.clone(),
                level: "error",
                rule: "cases-file",
                message: format!("数据文件不存在: {data_file}"),
            });
        }
    }

    // 已知变量：base + 集合链 + 数据驱动行 + 全部用例脚本写入（跨用例传递）
    let mut vars = base.clone();
    for c in &dc.chain {
        vars.push_str_layer(c.vars.plain());
    }
    if let Some(spec) = &case.cases {
        match spec.resolve(dc.path.parent().unwrap_or(Path::new("."))) {
            // 各行键的并集都算已定义：行变量只在本用例内可见
            Ok(rows) => rows.into_iter().for_each(|row| vars.push_layer(row)),
            // 文件不存在已由上面的 cases-file 报告；这里只报解析失败
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                issues.push(ValidationIssue {
                    fixable: false,
                    file: file.clone(),
                    level: "error",
                    rule: "cases-file",
                    message: format!("数据文件解析失败: {e}"),
                })
            }
            Err(_) => {}
        }
    }
    // multipart 上传文件存在性（含 {{var}} 的路径运行时才知道，跳过）
    if let Some(crate::config::BodySpec::Multipart { multipart }) = case.request.as_ref().and_then(|r| r.body.as_ref()) {
        for (field, v) in multipart {
            if let crate::config::MultipartValue::File { file: f, .. } = v {
                if !f.contains("{{") && !dc.path.parent().unwrap_or(Path::new(".")).join(f).is_file() {
                    issues.push(ValidationIssue {
                        fixable: false,
                        file: file.clone(),
                        level: "error",
                        rule: "multipart-file",
                        message: format!("multipart 字段 {field} 的上传文件不存在: {f}（路径相对用例文件）"),
                    });
                }
            }
        }
    }

    let mut known = vars.env_snapshot();
    for k in global_written {
        known.entry(k.clone()).or_insert_with(|| VarVal::Str(String::new()));
    }
    let known_vars = {
        let mut v = Vars::new();
        v.push_layer(known);
        v
    };

    let mut texts = case.request_texts();
    if let Some(e) = &case.expect {
        texts.extend(crate::expect::referenced_texts(e));
    }
    let mut reported: Vec<String> = vec![];
    for t in texts {
        for u in find_undefined(&t, &known_vars) {
            if reported.contains(&u) {
                continue;
            }
            reported.push(u.clone());
            issues.push(ValidationIssue {
                fixable: false,
                file: file.clone(),
                level: "warn",
                rule: "undefined-var",
                message: format!("变量 {{{{{u}}}}} 无定义来源（env/collection/脚本写入均未发现）"),
            });
        }
    }

    check_expect(&case, &file, issues);
    check_protocol(&case, dc, &file, issues);
    if let Some(doc) = std::fs::read_to_string(&dc.path).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) {
        push_advice(&file, crate::teach::upgrade_case(&doc, ctx).1, issues);
    }

    if !case.pre.is_empty() {
        if let Err(e) = host.compile_check(&case.pre) {
            issues.push(ValidationIssue {
                fixable: false,
                file: file.clone(),
                level: "error",
                rule: "script-syntax",
                message: format!("pre: {e:#}"),
            });
        }
    }
    if !case.post.is_empty() {
        if let Err(e) = host.compile_check(&case.post) {
            issues.push(ValidationIssue {
                fixable: false,
                file: file.clone(),
                level: "error",
                rule: "script-syntax",
                message: format!("post: {e:#}"),
            });
        }
    }
    for (i, t) in case.tests.iter().enumerate() {
        if let Err(e) = host.compile_check(&format!("return ({t})")) {
            issues.push(ValidationIssue {
                fixable: false,
                file: file.clone(),
                level: "error",
                rule: "script-syntax",
                message: format!("tests[{i}]: {e:#}"),
            });
        }
    }
}

/// 协议相关：request / grpc / ws 恰好一个；gRPC 地址与方法（配置了 .proto 时离线核对方法与消息字段）；WebSocket 步骤
fn check_protocol(case: &TestCase, dc: &DiscoveredCase, file: &str, issues: &mut Vec<ValidationIssue>) {
    use crate::config::{Protocol, WsAction};
    let mut push = |rule: &'static str, message: String| {
        issues.push(ValidationIssue { file: file.to_string(), level: "error", rule, message, fixable: false });
    };
    if let Some(e) = case.spec_error() {
        push("protocol", e);
        return;
    }
    let has_var = |t: &str| t.contains("{{");
    // 期望状态与协议相符（变量形态运行时才知道）
    if let Some(st) = case.expected_status() {
        if !matches!(st, crate::config::case::ExpectStatus::Var(t) if has_var(t)) {
            if let Err(e) = st.resolve_for(case.protocol(), &Vars::new()) {
                push("expect-status", format!("expect.status：{e}"));
            }
        }
    }
    match case.protocol() {
        Protocol::Http => {}
        Protocol::Grpc => {
            let g = case.grpc.as_ref().unwrap();
            if g.message.is_some() && g.messages.is_some() {
                push("grpc-message", "grpc.message 与 grpc.messages 只能写一个：一元 / 服务端流写 message，客户端流 / 双向流写 messages".into());
            }
            if !has_var(&g.target) {
                if let Err(e) = crate::engine::grpc::parse_target(&g.target) {
                    push("grpc-target", format!("grpc.target：{e}"));
                }
            }
            if has_var(&g.method) {
                return;
            }
            if let Err(e) = crate::engine::grpc::split_method(&g.method) {
                push("grpc-method", format!("grpc.method：{e}"));
                return;
            }
            // 配置了 .proto：不连服务就能确认方法存在、消息字段写对
            let Some(root) = discover::find_project_root(&dc.path) else { return };
            let cfg = discover::project_file(&root).and_then(|p| load_strict::<crate::config::Project>(&p).ok()).map(|p| p.grpc).unwrap_or_default();
            match crate::engine::grpc::Source::from_config(&root, &cfg) {
                Ok(src @ crate::engine::grpc::Source::Protos { .. }) => {
                    let msgs: Vec<String> = g.outgoing().iter().map(|m| m.to_string()).filter(|m| !has_var(m)).collect();
                    match crate::engine::grpc::check_offline(&g.method, &msgs, &src) {
                        Ok(info) => {
                            if !info.client_streaming && g.messages.as_ref().is_some_and(|m| m.len() != 1) {
                                push("grpc-message", format!("{} 是{}方法，只能发一条消息：写 message", g.method, info.kind_label()));
                            }
                        }
                        Err(e) => push("grpc-proto", format!("{e:#}")),
                    }
                }
                Ok(_) => {}
                Err(e) => push("grpc-proto", format!("{e:#}")),
            }
        }
        Protocol::Web | Protocol::Android => check_ui(case, &mut push),
        Protocol::Ws => {
            let w = case.ws.as_ref().unwrap();
            if !has_var(&w.url) {
                if let Err(e) = crate::engine::ws::normalize_url(&w.url) {
                    push("ws-url", format!("ws.url：{e}"));
                }
            }
            for (i, step) in w.steps.iter().enumerate() {
                let n = i + 1;
                match step.action() {
                    None if step.actions().is_empty() => push("ws-step", format!("ws.steps 第 {n} 步没有动作：写 send / sendBinary / receive / waitFor / wait / close 其中一个")),
                    None => push("ws-step", format!("ws.steps 第 {n} 步写了多个动作（{}）：每一步只做一件事，拆成多步", step.actions().iter().map(|a| a.key()).collect::<Vec<_>>().join("、"))),
                    Some(a) => {
                        if !step.capture.is_empty() && !matches!(a, WsAction::Receive | WsAction::WaitFor) {
                            push("ws-step", format!("ws.steps 第 {n} 步：capture 只能写在 receive / waitFor 步骤上"));
                        }
                        for (path, want) in [&step.receive, &step.wait_for].into_iter().flatten().flatten() {
                            if let Some(err) = crate::expect::lint_path(path) {
                                push("expect-path", format!("ws.steps 第 {n} 步：{err}"));
                            }
                            for err in crate::expect::lint_matcher(want) {
                                push("expect-matcher", format!("ws.steps 第 {n} 步 {path}：{err}（requestbaby guide ws）"));
                            }
                        }
                        for (name, from) in &step.capture {
                            match from.as_str() {
                                Some(f) if crate::expect::lint_path(f).is_none() && !f.contains('*') => {}
                                _ => push("capture-source", format!("ws.steps 第 {n} 步 capture.{name}：写消息里的路径（如 data.id），不能含 [*]")),
                            }
                        }
                    }
                }
            }
        }
    }
}

/// 界面用例：地址 / 包名、每一步恰好一个动作且适用于该端、定位写法、动作参数
fn check_ui(case: &TestCase, push: &mut dyn FnMut(&'static str, String)) {
    use crate::config::{CaptureSource, Locator, UiAction};
    let has_var = |t: &str| t.contains("{{");
    let android = case.android.is_some();
    let (key, steps) = match (&case.web, &case.android) {
        (Some(w), _) => {
            let u = w.url.trim();
            if u.is_empty() {
                push("web-url", "web.url 不能为空：写要打开的页面地址".into());
            } else if !has_var(u) && !["http://", "https://", "file://", "about:", "data:"].iter().any(|p| u.to_ascii_lowercase().starts_with(p)) {
                push("web-url", format!("web.url 写完整地址（http:// 或 https:// 开头），实际 {u}"));
            }
            if let Some(v) = &w.viewport {
                if v.width < 200 || v.height < 200 || v.width > 7680 || v.height > 4320 {
                    push("web-viewport", format!("web.viewport {}×{} 不合理（宽高 200 ~ 7680）", v.width, v.height));
                }
            }
            ("web", &w.steps)
        }
        (_, Some(a)) => {
            let app = a.app.trim();
            if app.is_empty() {
                push("android-app", "android.app 不能为空：写应用包名，如 com.example.shop".into());
            } else if !has_var(app) && (!app.contains('.') || app.contains(char::is_whitespace) || app.contains('/')) {
                push("android-app", format!("android.app 写应用包名（如 com.example.shop），实际 {app}"));
            }
            ("android", &a.steps)
        }
        _ => return,
    };
    let side = if android { "Android" } else { "Web" };
    let lint_locator = |l: &Locator, at: &str, condition: bool, push: &mut dyn FnMut(&'static str, String)| {
        let primary: Vec<&str> = [("role", l.role.is_some()), ("label", l.label.is_some()), ("text", l.text.is_some()), ("placeholder", l.placeholder.is_some()), ("testId", l.test_id.is_some()), ("css", l.css.is_some()), ("id", l.id.is_some()), ("desc", l.desc.is_some())]
            .into_iter()
            .filter(|(_, on)| *on)
            .map(|(k, _)| k)
            .collect();
        // Android 的 id + text 组合是常见写法（同 id 的列表项按文字区分），其余只写一种
        let combo_ok = android && primary.len() == 2 && primary.contains(&"id");
        if primary.len() > 1 && !combo_ok {
            push("ui-locator", format!("{at}：定位只写一种（{}），需要更精确时用 nth / exact / hasText", primary.join("、")));
        }
        if l.name.is_some() && l.role.is_none() {
            push("ui-locator", format!("{at}：name 要和 role 一起写（{{\"role\": \"button\", \"name\": \"登录\"}}）；只按文字找写 text"));
        }
        let wrong: Vec<&str> = if android {
            [("css", l.css.is_some()), ("testId", l.test_id.is_some()), ("placeholder", l.placeholder.is_some()), ("url", l.url.is_some()), ("title", l.title.is_some())].into_iter().filter(|(_, on)| *on).map(|(k, _)| k).collect()
        } else {
            [("id", l.id.is_some()), ("desc", l.desc.is_some()), ("activity", l.activity.is_some())].into_iter().filter(|(_, on)| *on).map(|(k, _)| k).collect()
        };
        if !wrong.is_empty() {
            push("ui-locator", format!("{at}：{} 用例不能写 {}（{}）", side, wrong.join("、"), if android { "Android 用 id / text / desc / label / role" } else { "Web 用 role+name / label / text / placeholder / testId / css" }));
        }
        let cond: Vec<&str> = [("url", l.url.is_some()), ("title", l.title.is_some()), ("activity", l.activity.is_some()), ("state", l.state.is_some()), ("count", l.count.is_some()), ("value", l.value.is_some())]
            .into_iter()
            .filter(|(_, on)| *on)
            .map(|(k, _)| k)
            .collect();
        if condition {
            if !l.has_target() && l.url.is_none() && l.title.is_none() && l.activity.is_none() {
                push("ui-locator", format!("{at}：写要等待 / 验证的元素（role+name / label / text …）或页面条件（{}）", if android { "activity" } else { "url / title" }));
            }
            if !l.has_target() && (l.state.is_some() || l.count.is_some() || l.value.is_some() || l.has_text.is_some()) {
                push("ui-locator", format!("{at}：state / count / value / hasText 是对元素的要求，要同时写元素定位"));
            }
        } else {
            if !l.has_target() {
                push("ui-locator", format!("{at}：写要操作的元素（{}）", if android { "id / text / desc / label / role" } else { "role+name / label / text / placeholder / testId / css" }));
            }
            if !cond.is_empty() {
                push("ui-locator", format!("{at}：{} 只能写在 waitFor / expect 里", cond.join("、")));
            }
        }
        if l.attr.is_some() || l.regex.is_some() {
            push("ui-locator", format!("{at}：attr / regex 只能写在 capture 里"));
        }
    };
    for (i, step) in steps.iter().enumerate() {
        let n = i + 1;
        let at = format!("{key}.steps 第 {n} 步");
        let Some(a) = step.action() else {
            let list = if android { "tap / fill / type / press / swipe / back / launch / stop / waitFor / expect / screenshot / wait" } else { "click / fill / select / press / hover / goto / back / waitFor / expect / screenshot / wait" };
            if step.actions().is_empty() {
                push("ui-step", format!("{at}没有动作：写 {list} 其中一个"));
            } else {
                push("ui-step", format!("{at}写了多个动作（{}）：每一步只做一件事，拆成多步", step.actions().iter().map(|a| a.key(android)).collect::<Vec<_>>().join("、")));
            }
            continue;
        };
        if !a.supported(android) {
            push("ui-step", format!("{at}：{} 是 {} 专用的动作", a.key(android), if android { "Web" } else { "Android" }));
            continue;
        }
        if let Some(l) = step.locator() {
            lint_locator(l, &at, matches!(a, UiAction::WaitFor | UiAction::Expect), push);
        }
        match (a, &step.value) {
            (UiAction::Fill | UiAction::Select, None) => push("ui-step", format!("{at}：{} 要写 value（{}）", a.key(android), if a == UiAction::Fill { "要填的内容" } else { "选项文字或值" })),
            (UiAction::Fill | UiAction::Select, Some(_)) => {}
            (_, Some(_)) => push("ui-step", format!("{at}：value 只用于 fill / select")),
            _ => {}
        }
        if step.clear_data.is_some() && a != UiAction::Launch {
            push("ui-step", format!("{at}：clearData 只用于 launch"));
        }
        if let Some(k) = step.press.as_deref().filter(|k| !has_var(k)) {
            let ok = if android { crate::engine::android::keycode(k).map(|_| ()) } else { crate::engine::web::key_event(k).map(|_| ()) };
            if let Err(e) = ok {
                push("ui-step", format!("{at}：{e:#}"));
            }
        }
        if let Some(d) = step.swipe.as_deref() {
            if !matches!(d, "up" | "down" | "left" | "right") {
                push("ui-step", format!("{at}：swipe 写 up / down / left / right（手指移动的方向）"));
            }
        }
        if let Some(name) = &step.screenshot {
            if name.trim().is_empty() {
                push("ui-step", format!("{at}：screenshot 写这张截图的名字（如 \"下单成功\"）"));
            }
        }
        for (name, src) in &step.capture {
            match src {
                CaptureSource::Builtin(b) => {
                    let ok = if android { b == "activity" } else { b == "url" || b == "title" };
                    if !ok {
                        push("capture-source", format!("{at} capture.{name}：{}", if android { "写 \"activity\" 或一个元素定位（{\"id\": \"order_no\"}）" } else { "写 \"url\" / \"title\" 或一个元素定位（{\"testId\": \"order-no\"}）" }));
                    }
                }
                CaptureSource::Element(l) => {
                    if !l.has_target() {
                        push("capture-source", format!("{at} capture.{name}：写要取值的元素定位"));
                    }
                    if let Some(re) = &l.regex {
                        if let Err(e) = regex::Regex::new(re) {
                            push("capture-source", format!("{at} capture.{name}：regex 写法有误：{e}"));
                        }
                    }
                }
            }
        }
    }
}

/// expect / capture 的写法：路径、运算符、来源；expectStatus 与 expect.status 不能同时写
fn check_expect(case: &TestCase, file: &str, issues: &mut Vec<ValidationIssue>) {
    let mut push = |rule: &'static str, message: String| {
        issues.push(ValidationIssue { file: file.to_string(), level: "error", rule, message, fixable: false });
    };
    if let Some(e) = &case.expect {
        if e.status.is_some() && case.expect_status.is_some() {
            push("expect-status-dup", "expectStatus（旧写法）与 expect.status 同时存在：删掉 expectStatus，只保留 expect.status（两者相同时 requestbaby fmt --fix 自动删除）".into());
        }
        for (path, want) in &e.body {
            if let Some(err) = crate::expect::lint_path(path) {
                push("expect-path", format!("expect.body：{err}"));
            }
            for err in crate::expect::lint_matcher(want) {
                push("expect-matcher", format!("expect.body.{path}：{err}（requestbaby guide expect）"));
            }
        }
        for (name, want) in &e.headers {
            for err in crate::expect::lint_matcher(want) {
                push("expect-matcher", format!("expect.headers.{name}：{err}（requestbaby guide expect）"));
            }
        }
    }
    for (name, from) in &case.capture {
        match from.as_str() {
            None => push("capture-source", format!("capture.{name} 必须是字符串：响应体路径（如 data.token）或 header:名称")),
            Some(f) if f.starts_with("header:") => {
                if f["header:".len()..].trim().is_empty() {
                    push("capture-source", format!("capture.{name}：header: 后面要写响应头名称"));
                }
            }
            Some(f) => {
                if let Some(err) = crate::expect::lint_path(f) {
                    push("capture-source", format!("capture.{name}：{err}"));
                } else if f.contains('*') {
                    push("capture-source", format!("capture.{name}：捕获路径不能包含 [*]，请写具体下标"));
                }
            }
        }
    }
}

/// 手工扫描脚本写入的 vars：匹配 vars.NAME = 或 vars["NAME"] =（无 regex 依赖）
pub fn scan_var_writes(src: &str) -> Vec<String> {
    let mut names = vec![];
    let bytes = src.as_bytes();
    let mut i = 0;
    while let Some(pos) = src[i..].find("vars") {
        let at = i + pos;
        i = at + 4;
        // 前界检查：排除 myvars / a.vars 等误匹配
        if at > 0 {
            let prev = bytes[at - 1];
            if prev.is_ascii_alphanumeric() || prev == b'_' || prev == b'$' || prev == b'.' {
                continue;
            }
        }
        let mut j = at + 4;
        let name = if bytes.get(j) == Some(&b'.') {
            j += 1;
            let start = j;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            Some(src[start..j].to_string())
        } else if bytes.get(j) == Some(&b'[') && bytes.get(j + 1) == Some(&b'"') {
            j += 2;
            let start = j;
            while j < bytes.len() && bytes[j] != b'"' {
                j += 1;
            }
            Some(src[start..j].to_string())
        } else {
            None
        };
        if let Some(n) = name {
            if !n.is_empty() && !names.contains(&n) {
                names.push(n);
            }
        }
    }
    names
}

/// 项目的校验范围：collections/ + 本环境会运行的流程引用到的 collections 之外的目录（如 fixtures/）；
/// 固定在其他环境的流程不算（它们的变量属于那个环境）。CLI `validate`（不带路径）、`verify`、桌面端校验面板共用
pub fn project_scope(root: &Path, env: &str) -> Vec<std::path::PathBuf> {
    let collections = root.join("collections");
    let mut targets = vec![if collections.is_dir() { collections.clone() } else { root.to_path_buf() }];
    let project = discover::project_file(root).and_then(|pf| load_strict::<Project>(&pf).ok());
    for wf in project.iter().flat_map(|p| p.workflows.values()).filter(|w| w.env.as_deref().is_none_or(|e| e == env)) {
        for step in &wf.steps {
            let target = root.join(&step.run);
            if !target.starts_with(&collections) && target.exists() && !targets.iter().any(|t| target.starts_with(t)) {
                targets.push(target);
            }
        }
    }
    targets
}

/// 在全项目上下文中校验单个文件：跨用例捕获的变量（如前序 post 写入的 token）不会误报未定义
pub fn validate_file(path: &Path, env_name: &str) -> Result<Vec<ValidationIssue>> {
    let Some(root) = discover::find_project_root(path) else {
        return validate_paths(&[path.to_path_buf()], env_name);
    };
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    // 校验范围：collections/ + 文件所在的顶层目录（如 fixtures/collections/… 的 fixtures/），
    // 否则 collections 之外的用例永远「没有问题」
    let canon_root = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
    let mut scope = vec![root.join("collections")];
    if let Some(top) = target.strip_prefix(&canon_root).ok().and_then(|rel| rel.components().next()) {
        let top = root.join(top.as_os_str());
        if top.is_dir() && !scope.contains(&top) {
            scope.push(top);
        }
    }
    scope.retain(|p| p.exists());
    let all = validate_paths(&scope, env_name)?;
    Ok(all
        .into_iter()
        // 问题路径相对项目根（与进程工作目录无关）
        .filter(|i| std::fs::canonicalize(root.join(&i.file)).map(|f| f == target).unwrap_or(false))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 项目不在进程工作目录下时，单文件校验仍能拿到该文件的问题（Desktop 从任意目录启动）
    #[test]
    fn validate_file_independent_of_cwd() {
        let dir = tempfile::tempdir().unwrap();
        crate::scaffold::init_project(dir.path()).unwrap();
        let case = dir.path().join("collections/demo/03-bad.json");
        std::fs::write(&case, r#"{"request":{"method":"GET","url":"{{baseUrl}}/{{nope}}"},"tests":["res.status &&"]}"#).unwrap();
        let issues = validate_file(&case, "dev").unwrap();
        let rules: Vec<&str> = issues.iter().map(|i| i.rule).collect();
        assert!(rules.contains(&"undefined-var"), "{rules:?}");
        assert!(rules.contains(&"script-syntax"), "{rules:?}");
        let clean = validate_file(&dir.path().join("collections/demo/01-health.json"), "dev").unwrap();
        assert!(clean.is_empty(), "{clean:?}");
    }

    /// collections/ 之外的用例（如 fixtures/collections/…）同样校验，不会误报「没有问题」
    #[test]
    fn validate_file_outside_collections() {
        let dir = tempfile::tempdir().unwrap();
        crate::scaffold::init_project(dir.path()).unwrap();
        let fx = dir.path().join("fixtures/collections/a");
        std::fs::create_dir_all(&fx).unwrap();
        // 依赖同一顶层目录里前一个用例捕获的变量：不误报
        std::fs::write(fx.join("01-login.json"), r#"{"request":{"method":"GET","url":"{{baseUrl}}/login"},"capture":{"tk":"data.token"}}"#).unwrap();
        let bad = fx.join("02-bad.json");
        std::fs::write(&bad, r#"{"request":{"method":"GET","url":"{{baseUrl}}/x?t={{tk}}&n={{nope}}"},"expect":{"body":{"code":{"lenn":1}}}}"#).unwrap();
        let issues = validate_file(&bad, "dev").unwrap();
        let rules: Vec<&str> = issues.iter().map(|i| i.rule).collect();
        assert_eq!(rules, ["undefined-var", "expect-matcher"], "{issues:?}");
        assert!(issues[0].message.contains("nope") && issues[1].message.contains("是不是想写 len"));
        assert!(validate_file(&fx.join("01-login.json"), "dev").unwrap().is_empty());
        // 集合配置文件也在范围内
        std::fs::write(fx.join("collection.json"), r#"{"oops":1}"#).unwrap();
        assert!(!validate_file(&fx.join("collection.json"), "dev").unwrap().is_empty());
    }

    /// 数据驱动行定义的变量对本用例可见：不应报未定义
    #[test]
    fn data_row_vars_are_defined() {
        let dir = tempfile::tempdir().unwrap();
        crate::scaffold::init_project(dir.path()).unwrap();
        let case = dir.path().join("collections/demo/03-rows.json");
        std::fs::write(
            &case,
            r#"{"request":{"method":"GET","url":"{{baseUrl}}/tasks/{{taskId}}?page={{pageNo}}"},
               "cases":[{"label":"a","taskId":11,"pageNo":0},{"label":"b","taskId":"12","pageNo":3}]}"#,
        )
        .unwrap();
        let issues = validate_file(&case, "dev").unwrap();
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn multipart_file_must_exist() {
        let dir = tempfile::tempdir().unwrap();
        crate::scaffold::init_project(dir.path()).unwrap();
        let case = dir.path().join("collections/demo/03-upload.json");
        std::fs::write(
            &case,
            r#"{"request":{"method":"POST","url":"{{baseUrl}}/up","body":{"multipart":{"a":{"file":"fixtures/a.txt"},"b":"x"}}}}"#,
        )
        .unwrap();
        let issues = validate_file(&case, "dev").unwrap();
        assert!(issues.iter().any(|i| i.rule == "multipart-file"), "{issues:?}");
        std::fs::create_dir_all(dir.path().join("collections/demo/fixtures")).unwrap();
        std::fs::write(dir.path().join("collections/demo/fixtures/a.txt"), "hi").unwrap();
        let issues = validate_file(&case, "dev").unwrap();
        assert!(issues.is_empty(), "{issues:?}");
    }

    /// 只在本机覆盖里定义的变量（如真实 token）不算未定义
    #[test]
    fn local_override_vars_are_defined() {
        let dir = tempfile::tempdir().unwrap();
        crate::scaffold::init_project(dir.path()).unwrap();
        std::fs::write(dir.path().join("envs/dev.local.json"), r#"{"vars":{"token":{"value":"x","secret":true}}}"#).unwrap();
        let case = dir.path().join("collections/demo/03-me.json");
        std::fs::write(&case, r#"{"request":{"method":"GET","url":"{{baseUrl}}/me","headers":{"Authorization":"Bearer {{token}}"}}}"#).unwrap();
        let issues = validate_file(&case, "dev").unwrap();
        assert!(issues.is_empty(), "{issues:?}");
    }

    /// 选了不存在的环境：明确报错，而不是当空环境继续
    #[test]
    fn missing_env_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        crate::scaffold::init_project(dir.path()).unwrap();
        let issues = validate_paths(&[dir.path().join("collections")], "staging").unwrap();
        let missing = issues.iter().find(|i| i.rule == "env-missing").expect("应报 env-missing");
        assert_eq!(missing.level, "error");
        assert_eq!(missing.file, "envs/staging.json");
        assert!(validate_paths(&[dir.path().join("collections")], "dev").unwrap().iter().all(|i| i.rule != "env-missing"));
    }

    fn issue(file: &str, level: &'static str, rule: &'static str, fixable: bool) -> ValidationIssue {
        ValidationIssue { file: file.into(), level, rule, message: format!("{rule} 的说明"), fixable }
    }

    #[test]
    fn style_advice_is_for_ai_not_humans() {
        let issues = vec![
            issue("collections/a.json", "warn", "long-js-test", false),
            issue("collections/a.json", "warn", "prefer-expect", true),
            issue("collections/b.json", "warn", "undefined-var", false),
            issue("collections/c.json", "error", "schema", false),
        ];
        // 写法建议不算警告；undefined-var 是真问题，人要看
        assert_eq!(issues.iter().filter(|i| i.is_style()).count(), 2);
        let brief = ai_brief(&issues, "dev");
        // 只交代 AI 要改的（fmt --fix 能改的、真问题都不在里面），按文件分组
        assert!(brief.contains("collections/a.json\n- [long-js-test]"), "{brief}");
        assert!(!brief.contains("prefer-expect") && !brief.contains("undefined-var") && !brief.contains("schema"), "{brief}");
        assert!(brief.contains("requestbaby validate -e dev"), "{brief}");
        assert!(ai_brief(&issues[1..], "").is_empty());
        assert!(ai_brief(&issues[..1], "").contains("`requestbaby validate`"));
    }
}
