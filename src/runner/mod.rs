pub mod discover;

use crate::config::{load_strict, Collection, Project, TestCase};
use crate::engine::{CurlTransport, ResolvedRequest, Response, Transport};
use crate::script::{QuickJsHost, ReqData, RespData, ScriptHost, TestOutcome};
use crate::vars::{VarVal, Vars};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::Path;

/// 运行选项
#[derive(Clone)]
pub struct RunOptions {
    pub tags: Vec<String>,
    pub name_filter: String,
    pub fail_fast: bool,
    pub dry_run: bool,
    pub verbose: bool,
    /// 响应落盘目录（-o）：每用例一个 JSON（meta + body）
    pub output_dir: Option<std::path::PathBuf>,
    /// send 模式：成功也填充 full_response（交互展示用）
    pub send_mode: bool,
    /// 并发档位：None 串行；Some(n) 独立用例并发（依赖捕获链/带脚本的仍串行在前）
    pub parallel: Option<usize>,
}

/// 失败时的响应快照（进 --json 报告，辅助 AI 自修复）
#[derive(Debug, Clone, serde::Serialize)]
pub struct ResponseSnapshot {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub duration_ms: u64,
}

/// 单用例执行结果
#[derive(Clone)]
pub struct CaseResult {
    pub rel: String,
    pub name: String,
    pub tags: Vec<String>,
    pub method: String,
    pub url: String,
    pub status: u16,
    pub duration_ms: u64,
    pub skipped: bool,
    pub error: Option<String>,
    pub tests: Vec<TestOutcome>,
    /// 编译出的 curl 命令（dry-run / -v 展示）
    pub curl_cmd: Option<String>,
    /// body 内容预览（dry-run 展示用，截断）
    pub body_preview: Option<String>,
    /// 失败时的响应快照
    pub response_snapshot: Option<ResponseSnapshot>,
    /// 完整响应（send 模式交互展示用）
    pub full_response: Option<crate::script::RespData>,
}

impl CaseResult {
    pub fn passed(&self) -> bool {
        !self.skipped && self.error.is_none() && self.tests.iter().all(|t| t.pass)
    }

    /// 错误/占位构造器（统一 5 处样板）
    pub fn placeholder(rel: &str, name: &str, error: Option<String>) -> Self {
        Self {
            rel: rel.to_string(),
            name: name.to_string(),
            tags: vec![],
            method: "-".into(),
            url: "-".into(),
            status: 0,
            duration_ms: 0,
            skipped: false,
            error,
            tests: vec![],
            curl_cmd: None,
            body_preview: None,
            response_snapshot: None,
            full_response: None,
        }
    }
}

/// 项目上下文（一次 run 内共享）
#[derive(Clone)]
pub struct ProjectCtx {
    pub project: Project,
    pub env_vars: BTreeMap<String, String>,
    /// 敏感变量值清单（输出层脱敏用）
    pub secret_values: Vec<String>,
}

/// 编排器：discover → filter → execute（顺序/并行）→ results
#[derive(Clone)]
pub struct Runner {
    ctx: ProjectCtx,
    /// 执行引擎（libcurl binding）
    transport: CurlTransport,
    script: QuickJsHost,
    runtime_vars: BTreeMap<String, VarVal>,
}

impl Runner {
    pub fn new(ctx: ProjectCtx) -> Self {
        Self {
            ctx,
            transport: CurlTransport::new(),
            script: QuickJsHost::new(),
            runtime_vars: BTreeMap::new(),
        }
    }

    /// 构建某个用例的变量作用域：project < env < 集合链 < runtime
    fn vars_for(&self, chain: &[Collection]) -> Vars {
        let mut v = Vars::new();
        v.push_str_layer(self.ctx.project.vars.plain());
        v.push_str_layer(self.ctx.env_vars.clone());
        for c in chain {
            v.push_str_layer(c.vars.plain());
        }
        v.push_layer(self.runtime_vars.clone());
        v
    }

    /// cur 相对 prev 新进入的目录层级（浅 → 深，含 cur 自身）
    fn entering_levels(prev: Option<&Path>, cur: &Path) -> Vec<std::path::PathBuf> {
        // 计算双方祖先链，取 cur 链中 prev 链前缀之后的部分
        let chain_of = |p: &Path| -> Vec<std::path::PathBuf> {
            let mut out = vec![];
            let mut acc = std::path::PathBuf::new();
            for seg in p.components() {
                use std::path::Component;
                match seg {
                    Component::RootDir | Component::Prefix(_) => {
                        acc.push(seg.as_os_str());
                    }
                    Component::Normal(n) => {
                        acc.push(n);
                        out.push(acc.clone());
                    }
                    _ => {}
                }
            }
            out
        };
        let cur_chain = chain_of(cur);
        let prev_len = prev
            .map(|p| {
                let pc = chain_of(p);
                // prev 是 cur 的祖先时：取其链长度；否则公共祖先长度
                let mut common = 0;
                for (a, b) in pc.iter().zip(cur_chain.iter()) {
                    if a == b {
                        common += 1;
                    } else {
                        break;
                    }
                }
                common
            })
            .unwrap_or(0);
        cur_chain[prev_len.min(cur_chain.len())..].to_vec()
    }

    /// name 过滤同时匹配用例名与文件名 stem（--name enable 能命中 03-enable.json）
    fn matches_filter(tags: &[String], name: &str, file_stem: &str, opts: &RunOptions) -> bool {
        if opts.tags.is_empty() && opts.name_filter.is_empty() {
            return true;
        }
        if !opts.tags.is_empty()
            && !opts
                .tags
                .iter()
                .any(|t| tags.iter().any(|x| x.eq_ignore_ascii_case(t)))
        {
            return false;
        }
        if !opts.name_filter.is_empty() {
            let kw = opts.name_filter.to_lowercase();
            if !name.to_lowercase().contains(&kw) && !file_stem.to_lowercase().contains(&kw) {
                return false;
            }
        }
        true
    }

    /// 执行全部发现用例：默认串行；--parallel N 时两阶段（依赖串行 → 独立并发）
    pub fn run(
        &mut self,
        cases: &[discover::DiscoveredCase],
        opts: &RunOptions,
        mut sink: impl FnMut(&CaseResult),
    ) -> Result<Vec<CaseResult>> {
        if opts.parallel.is_some() && !opts.dry_run {
            return self.run_parallel(cases, opts, sink);
        }
        self.run_serial(cases, opts, &mut sink)
    }

    /// 两阶段并行：带脚本或消费捕获变量的用例串行在前，其余按 chunk 并发
    fn run_parallel(
        &mut self,
        cases: &[discover::DiscoveredCase],
        opts: &RunOptions,
        mut sink: impl FnMut(&CaseResult),
    ) -> Result<Vec<CaseResult>> {
        let width = opts.parallel.unwrap().max(1);
        // 收集全部脚本写入的变量名（跨用例依赖判定）
        let mut written: Vec<String> = vec![];
        for dc in cases {
            if let Ok(c) = load_strict::<TestCase>(&dc.path) {
                for n in crate::validate::scan_var_writes(&c.pre)
                    .into_iter()
                    .chain(crate::validate::scan_var_writes(&c.post))
                {
                    if !written.contains(&n) {
                        written.push(n);
                    }
                }
            }
        }
        let consumes_written = |c: &TestCase| -> bool {
            let mut texts = vec![c.request.url.clone()];
            texts.extend(c.request.headers.values().cloned());
            if let Some(b) = &c.request.body {
                texts.push(b.render());
            }
            texts.extend(c.tests.iter().cloned());
            texts.push(c.pre.clone());
            texts.push(c.post.clone());
            let mut found = false;
            for t in texts {
                for w in &written {
                    let ph = format!("{{{{{w}}}}}");
                    let env_ref = format!("env.{w}");
                    if t.contains(&ph) || t.contains(&env_ref) {
                        found = true;
                    }
                }
            }
            found
        };

        // 过滤 + 分类（保持原序）
        let mut serial: Vec<&discover::DiscoveredCase> = vec![];
        let mut parallel: Vec<&discover::DiscoveredCase> = vec![];
        for dc in cases {
            let Ok(case) = load_strict::<TestCase>(&dc.path) else { continue };
            let mut all_tags: Vec<String> = dc.chain.iter().flat_map(|c| c.tags.clone()).collect();
            all_tags.extend(case.tags.iter().cloned());
            let name = if case.name.is_empty() {
                dc.path.file_stem().unwrap_or_default().to_string_lossy().into_owned()
            } else {
                case.name.clone()
            };
            let stem = dc.path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            if !Self::matches_filter(&all_tags, &name, &stem, opts) {
                continue;
            }
            let depends = !case.pre.is_empty() || !case.post.is_empty() || case.cases.is_some() || consumes_written(&case);
            if depends {
                serial.push(dc);
            } else {
                parallel.push(dc);
            }
        }

        let mut results: Vec<CaseResult> = vec![];
        // 阶段1：串行（含 setup/teardown 跟踪）
        if !serial.is_empty() {
            let ser: Vec<discover::DiscoveredCase> = serial.into_iter().cloned().collect();
            let mut sub_opts = opts.clone();
            sub_opts.parallel = None;
            let rs = self.run_serial(&ser, &sub_opts, &mut |r| sink(r))?;
            let stop = opts.fail_fast && rs.iter().any(|r| !r.passed());
            results.extend(rs);
            if stop {
                return Ok(results);
            }
        }
        // 阶段2：独立用例并发（跳过 setup/teardown；fail_fast 不中断进行中批次）
        for chunk in parallel.chunks(width) {
            let mut handles = vec![];
            for dc in chunk {
                let dc: discover::DiscoveredCase = (*dc).clone();
                let runner = self.clone();
                let o = RunOptions {
                    tags: vec![], name_filter: String::new(), fail_fast: false,
                    dry_run: false, verbose: opts.verbose,
                    output_dir: opts.output_dir.clone(), send_mode: false, parallel: None,
                };
                handles.push(std::thread::spawn(move || {
                    let mut r = runner;
                    let mut out: Vec<CaseResult> = vec![];
                    let _ = r.run_serial(std::slice::from_ref(&dc), &o, &mut |x| out.push(x.clone()));
                    out
                }));
            }
            let mut chunk_failed = false;
            for h in handles {
                if let Ok(rs) = h.join() {
                    for r in rs {
                        if !r.passed() {
                            chunk_failed = true;
                        }
                        sink(&r);
                        results.push(r);
                    }
                }
            }
            if opts.fail_fast && chunk_failed {
                break;
            }
        }
        Ok(results)
    }

    /// 执行全部发现用例（按 rel 路径序；目录进出触发集合 setup/teardown）
    ///
    /// 副作用隔离：先按 filters 计划存活用例，setup/teardown 仅为存活用例的目录执行
    /// —— `--tag smoke` 只读跑法不会触发其他集合的造数脚本
    fn run_serial(
        &mut self,
        cases: &[discover::DiscoveredCase],
        opts: &RunOptions,
        sink: &mut dyn FnMut(&CaseResult),
    ) -> Result<Vec<CaseResult>> {
        let mut results = vec![];
        let mut prev_dir: Option<std::path::PathBuf> = None;
        let mut active_dirs: Vec<std::path::PathBuf> = vec![]; // 当前进入的目录层级（浅→深）

        // ---- 计划阶段：加载 + 过滤（加载失败保留执行，报告配置错误）----
        let mut planned: Vec<(&discover::DiscoveredCase, Option<TestCase>)> = vec![];
        for dc in cases {
            match load_strict::<TestCase>(&dc.path) {
                Ok(case) => {
                    let mut all_tags: Vec<String> =
                        dc.chain.iter().flat_map(|c| c.tags.clone()).collect();
                    all_tags.extend(case.tags.iter().cloned());
                    let name = display_name(dc, &case);
                    let stem = dc
                        .path
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    if Self::matches_filter(&all_tags, &name, &stem, opts) {
                        planned.push((dc, Some(case)));
                    }
                }
                Err(_) => planned.push((dc, None)),
            }
        }

        for (dc, case_opt) in planned {
            let case_dir = dc
                .path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_default();

            // ---- 进入新目录层级：执行新增层级的 setup（浅 → 深）----
            if prev_dir.as_deref() != Some(case_dir.as_path()) {
                let new_levels = Self::entering_levels(prev_dir.as_deref(), &case_dir);
                for lvl in &new_levels {
                    active_dirs.push(lvl.clone());
                    if !opts.dry_run {
                        if let Some(r) = self.run_collection_script(lvl, "setup") {
                            sink(&r);
                            results.push(r);
                            if opts.fail_fast {
                                return self.finish_teardown(opts, active_dirs, results);
                            }
                        }
                    }
                }
                prev_dir = Some(case_dir);
            }

            let case = match case_opt {
                Some(c) => c,
                None => {
                    let e = load_strict::<TestCase>(&dc.path)
                        .err()
                        .map(|e| format!("{e:#}"))
                        .unwrap_or_else(|| "配置错误".into());
                    let name = dc
                        .path
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    let r = CaseResult::placeholder(&dc.rel, &name, Some(e));
                    sink(&r);
                    results.push(r);
                    if opts.fail_fast {
                        break;
                    }
                    continue;
                }
            };

            // 合并集合级 tags，计算展示名
            let mut all_tags: Vec<String> = dc.chain.iter().flat_map(|c| c.tags.clone()).collect();
            all_tags.extend(case.tags.iter().cloned());
            let name = display_name(dc, &case);

            // ---- 数据驱动展开（用例 × 行）----
            let rows: Vec<Option<BTreeMap<String, String>>> = match &case.cases {
                None => vec![None],
                Some(spec) => {
                    let base = dc.path.parent().unwrap_or(std::path::Path::new("."));
                    match spec.resolve(base) {
                        Ok(rs) if !rs.is_empty() => rs.into_iter().map(Some).collect(),
                        Ok(_) => vec![None],
                        Err(e) => {
                            let r = self.error_result(dc, &name, &all_tags, format!("数据文件加载失败: {e}"));
                            sink(&r);
                            results.push(r);
                            if opts.fail_fast {
                                break;
                            }
                            continue;
                        }
                    }
                }
            };

            let mut stop = false;
            for (i, row) in rows.iter().enumerate() {
                let inst_name = match (&case.cases, row) {
                    (Some(_), Some(r)) => {
                        let label = r.get("label").cloned().unwrap_or_else(|| format!("#{}", i));
                        format!("{name} [{label}]")
                    }
                    _ => name.clone(),
                };
                let r = self.execute_case(dc, &case, &all_tags, &inst_name, row.as_ref(), opts);
                let bad = !r.passed();
                sink(&r);
                results.push(r);
                if opts.fail_fast && bad {
                    stop = true;
                    break;
                }
            }
            if stop {
                break;
            }
        }

        self.finish_teardown(opts, active_dirs, results)
    }

    /// teardown：活跃层级深 → 浅，无论成败执行（失败仅 stderr 提示）
    fn finish_teardown(
        &mut self,
        opts: &RunOptions,
        active_dirs: Vec<std::path::PathBuf>,
        results: Vec<CaseResult>,
    ) -> Result<Vec<CaseResult>> {
        if !opts.dry_run {
            for lvl in active_dirs.iter().rev() {
                if let Some(r) = self.run_collection_script(lvl, "teardown") {
                    eprintln!(
                        "\x1b[33m[teardown] {} {}\x1b[0m",
                        lvl.display(),
                        r.error.as_deref().unwrap_or("")
                    );
                }
            }
        }
        Ok(results)
    }

    /// 执行某目录 collection.json 的 setup/teardown；失败返回合成结果
    fn run_collection_script(&mut self, dir: &Path, kind: &str) -> Option<CaseResult> {
        let cfg_path = dir.join("collection.json");
        let Ok(col) = load_strict::<Collection>(&cfg_path) else {
            return None;
        };
        let script = match kind {
            "setup" => &col.setup,
            _ => &col.teardown,
        };
        if script.is_empty() {
            return None;
        }
        let display_name = if col.name.is_empty() {
            dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
        } else {
            col.name.clone()
        };
        let env = {
            let mut v = Vars::new();
            v.push_str_layer(self.ctx.project.vars.plain());
            v.push_str_layer(self.ctx.env_vars.clone());
            v.push_str_layer(col.vars.plain());
            v.push_layer(self.runtime_vars.clone());
            v.env_snapshot()
        };
        match self.script.run_scoped(&self.runtime_vars, &env, script) {
            Ok(out) => {
                if let Some(e) = out.error {
                    return Some(CaseResult {
                        rel: format!("({kind}) {}", dir.display()),
                        name: format!("({kind}) {display_name}"),
                        tags: vec![],
                        method: "-".into(),
                        url: "-".into(),
                        status: 0,
                        duration_ms: 0,
                        skipped: false,
                                    error: Some(format!("{kind} 脚本失败: {e}")),
                        tests: vec![],
                        curl_cmd: None,
                        body_preview: None,
                        response_snapshot: None,
                        full_response: None,
                    });
                }
                self.runtime_vars.extend(out.vars);
            }
            Err(e) => {
                return Some(CaseResult::placeholder(
                    &format!("({kind}) {}", dir.display()),
                    &format!("({kind}) {display_name}"),
                    Some(format!("{kind} 宿主错误: {e:#}")),
                ));
            }
        }
        None
    }

    fn error_result(&self, dc: &discover::DiscoveredCase, name: &str, tags: &[String], msg: String) -> CaseResult {
        CaseResult {
            rel: dc.rel.clone(),
            name: name.to_string(),
            tags: tags.to_vec(),
            method: "-".into(),
            url: "-".into(),
            status: 0,
            duration_ms: 0,
            skipped: false,
            error: Some(msg),
            tests: vec![],
            curl_cmd: None,
            body_preview: None,
            response_snapshot: None,
            full_response: None,
        }
    }

    fn execute_case(
        &mut self,
        dc: &discover::DiscoveredCase,
        case: &TestCase,
        tags: &[String],
        name: &str,
        row: Option<&BTreeMap<String, String>>,
        opts: &RunOptions,
    ) -> CaseResult {
        let mut vars = self.vars_for(&dc.chain);
        // 数据驱动行变量：优先级最高
        if let Some(row) = row {
            vars.push_str_layer(row.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        let mut result = CaseResult {
            rel: dc.rel.clone(),
            name: name.to_string(),
            tags: tags.to_vec(),
            method: case.request.method.clone(),
            url: case.request.url.clone(),
            status: 0,
            duration_ms: 0,
            skipped: case.skip,
            error: None,
            tests: vec![],
            curl_cmd: None,
            body_preview: None,
            response_snapshot: None,
            full_response: None,
        };
        if case.skip {
            return result;
        }

        // ---- 构建请求（变量替换 + headers 继承合并）----
        let timeout = case.request.timeout_ms.unwrap_or(self.ctx.project.timeout_ms);
        // 集合链 headers（浅→深）→ 用例 headers 覆盖（key 小写归一防重复发送）
        let mut merged_headers: BTreeMap<String, (String, String)> = BTreeMap::new();
        for c in &dc.chain {
            for (k, v) in &c.headers {
                merged_headers.insert(k.to_lowercase(), (k.clone(), v.clone()));
            }
        }
        for (k, v) in &case.request.headers {
            merged_headers.insert(k.to_lowercase(), (k.clone(), v.clone()));
        }
        let mut headers: Vec<(String, String)> = merged_headers
            .values()
            .map(|(k, v)| (k.clone(), vars.render(v)))
            .collect();
        let body_str = case.request.body.as_ref().map(|b| vars.render(&b.render()));
        if body_str.is_some()
            && case.request.body.as_ref().map(|b| b.is_json()).unwrap_or(false)
            && !merged_headers.contains_key("content-type")
        {
            headers.push(("Content-Type".into(), "application/json".into()));
        }
        let mut req = ResolvedRequest {
            method: case.request.method.clone(),
            url: vars.render(&case.request.url),
            headers,
            body: body_str,
            timeout_ms: timeout,
            retry: case.request.retry.unwrap_or(0),
            insecure: case.request.insecure.unwrap_or(false),
            follow_redirects: case.request.follow_redirects.unwrap_or(false),
        };

        let env_view = vars.env_snapshot();

        // ---- pre 脚本 ----
        if !case.pre.is_empty() {
            let req_data = ReqData {
                method: req.method.clone(),
                url: req.url.clone(),
                headers: req.headers.iter().cloned().collect(),
                body: req.body.clone(),
            };
            match self
                .script
                .run_pre(req_data, &self.runtime_vars, &env_view, &case.pre)
            {
                Ok(out) => {
                    if let Some(e) = out.error {
                        result.error = Some(format!("pre 脚本失败: {e}"));
                        return result;
                    }
                    if let Some(rd) = out.req {
                        req.method = rd.method;
                        req.url = rd.url;
                        req.headers = rd.headers.into_iter().collect();
                        req.body = rd.body;
                    }
                    self.runtime_vars.extend(out.vars);
                    if opts.verbose && !out.logs.is_empty() {
                        eprintln!("  [log] {}", self.redact_str(&out.logs.join(" | ")));
                    }
                }
                Err(e) => {
                    result.error = Some(format!("pre 脚本宿主错误: {e:#}"));
                    return result;
                }
            }
            // pre 写入的新变量参与最终渲染
            let vars2 = self.vars_for(&dc.chain);
            req.url = vars2.render(&req.url);
            req.headers = req
                .headers
                .iter()
                .map(|(k, v)| (k.clone(), vars2.render(v)))
                .collect();
            if let Some(b) = &req.body {
                req.body = Some(vars2.render(b));
            }
        }

        result.url = req.url.clone();

        // ---- 残留占位符检查：真实执行时未定义变量提前清晰报错 ----
        // （dry-run 跳过：运行时捕获变量在前序未执行时残留属预期审阅内容）
        if !opts.dry_run {
            let mut texts = vec![req.url.clone()];
            texts.extend(req.headers.iter().map(|(_, v)| v.clone()));
            if let Some(b) = &req.body {
                texts.push(b.clone());
            }
            let mut leftovers: Vec<String> = vec![];
            for t in &texts {
                for lo in scan_leftover_placeholders(t) {
                    if !leftovers.contains(&lo) {
                        leftovers.push(lo);
                    }
                }
            }
            if !leftovers.is_empty() {
                let detail: Vec<String> = leftovers
                    .iter()
                    .map(|lo| {
                        if let Some(name) = lo.strip_prefix("%") {
                            format!(
                                "环境变量 {name} 未设置（{{{{{lo}}}}} 依赖进程环境变量，请 export {name} 或经 run-e2e.sh 运行）"
                            )
                        } else {
                            format!(
                                "变量 {lo} 未定义（检查 requestbaby.json / envs / collection vars / 前序用例 post 捕获）"
                            )
                        }
                    })
                    .collect();
                result.error = Some(format!("占位符未替换: {}", detail.join("; ")));
                return result;
            }
        }

        // ---- 编译 curl 命令展示（dry-run / -v）----
        let inv = match self.transport.compile(&req) {
            Ok(i) => i,
            Err(e) => {
                result.error = Some(format!("编译 curl 失败: {e:#}"));
                return result;
            }
        };
        result.curl_cmd = Some(inv.argv.join(" "));

        if opts.dry_run {
            if let Some(content) = &inv.body {
                let mut preview: String = content.chars().take(800).collect();
                if content.len() > 800 {
                    preview.push('…');
                }
                result.body_preview = Some(preview);
            }
            self.redact_result(&mut result);
            return result;
        }

        // ---- 执行 ----
        let resp: Response = match self.transport.execute(&req) {
            Ok(r) => r,
            Err(e) => {
                result.error = Some(format!("{e:#}"));
                return result;
            }
        };
        result.status = resp.status;
        result.duration_ms = resp.time_ms;

        if let Some(e) = &resp.transport_error {
            result.error = Some(format!("传输错误: {e}"));
            self.redact_result(&mut result);
            return result;
        }

        let resp_data = RespData {
            status: resp.status,
            time_ms: resp.time_ms,
            size: resp.size,
            headers: resp.headers.clone(),
            body: resp.body_text(),
        };

        // ---- post 脚本 ----
        if !case.post.is_empty() {
            let mut runtime = self.runtime_vars.clone();
            match self
                .script
                .run_post(&resp_data, &runtime, &env_view, &case.post)
            {
                Ok(out) => {
                    if let Some(e) = out.error {
                        result.error = Some(format!("post 脚本失败: {e}"));
                    } else {
                        runtime.extend(out.vars);
                        self.runtime_vars = runtime;
                    }
                    if opts.verbose && !out.logs.is_empty() {
                        eprintln!("  [log] {}", self.redact_str(&out.logs.join(" | ")));
                    }
                }
                Err(e) => {
                    result.error = Some(format!("post 脚本宿主错误: {e:#}"));
                }
            }
        }

        // ---- tests ----
        // expectStatus 语法糖 → 隐式首条断言（先于业务断言，快速失败）
        let mut exprs: Vec<String> = vec![];
        if let Some(es) = &case.expect_status {
            let list = es.to_list();
            exprs.push(format!(
                "[{}].includes(res.status)",
                list.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",")
            ));
        }
        exprs.extend(case.tests.iter().cloned());
        if !exprs.is_empty() {
            let mut env_vars = self.vars_for(&dc.chain);
            if let Some(row) = row {
                env_vars.push_str_layer(row.iter().map(|(k, v)| (k.clone(), v.clone())));
            }
            let env_now = env_vars.env_snapshot();
            match self
                .script
                .eval_tests(&resp_data, &self.runtime_vars, &env_now, &exprs)
            {
                Ok(ts) => result.tests = ts,
                Err(e) => result.error = Some(format!("tests 宿主错误: {e:#}")),
            }
        }

        // ---- 收尾：send 快照 / 失败快照 / 响应落盘（-o）/ 输出脱敏 ----
        self.finalize_result(&mut result, dc, &resp_data, opts);
        result
    }

    fn finalize_result(
        &self,
        result: &mut CaseResult,
        dc: &discover::DiscoveredCase,
        resp_data: &RespData,
        opts: &RunOptions,
    ) {
        if opts.send_mode {
            result.full_response = Some(resp_data.clone());
        }
        if !result.passed() {
            result.response_snapshot = Some(ResponseSnapshot {
                status: resp_data.status,
                headers: resp_data.headers.clone(),
                body: resp_data.body.chars().take(4000).collect::<String>(),
                duration_ms: resp_data.time_ms,
            });
        }
        if let Some(dir) = &opts.output_dir {
            let file = dir.join(format!("{}.resp.json", dc.rel.trim_end_matches(".json")));
            if let Some(parent) = file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let payload = serde_json::json!({
                "case": dc.rel,
                "name": &result.name,
                "status": resp_data.status,
                "durationMs": resp_data.time_ms,
                "headers": resp_data.headers,
                "body": resp_data.body,
            });
            let _ = std::fs::write(&file, serde_json::to_vec_pretty(&payload).unwrap_or_default());
        }
        self.redact_result(result);
    }

    /// 单段文本脱敏
    pub fn redact_str(&self, s: &str) -> String {
        let mut out = s.to_string();
        for sv in &self.ctx.secret_values {
            if !sv.is_empty() && out.contains(sv.as_str()) {
                out = out.replace(sv.as_str(), "****");
            }
        }
        out
    }

    /// 输出层脱敏：secret 变量值在 curl 命令/URL/错误/预览/快照/完整响应中一律替换为 ****
    fn redact_result(&self, result: &mut CaseResult) {
        if self.ctx.secret_values.is_empty() {
            return;
        }
        result.url = self.redact_str(&result.url);
        if let Some(c) = &mut result.curl_cmd {
            *c = self.redact_str(c);
        }
        if let Some(b) = &mut result.body_preview {
            *b = self.redact_str(b);
        }
        if let Some(e) = &mut result.error {
            *e = self.redact_str(e);
        }
        if let Some(snap) = &mut result.response_snapshot {
            snap.body = self.redact_str(&snap.body);
            for (_k, v) in &mut snap.headers {
                *v = self.redact_str(v);
            }
        }
        if let Some(fr) = &mut result.full_response {
            fr.body = self.redact_str(&fr.body);
            for (_k, v) in &mut fr.headers {
                *v = self.redact_str(v);
            }
        }
    }
}

/// 扫描文本中残留的 {{...}} 占位符（返回内部表达式，如 %ENV / name / name:number）
pub fn scan_leftover_placeholders(text: &str) -> Vec<String> {
    let mut out = vec![];
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        if let Some(end) = after.find("}}") {
            let inner = after[..end].trim().to_string();
            if !inner.is_empty() && !out.contains(&inner) {
                out.push(inner);
            }
            rest = &after[end + 2..];
        } else {
            break;
        }
    }
    out
}

/// 用例展示名：name 字段缺省回落文件名 stem
fn display_name(dc: &discover::DiscoveredCase, case: &TestCase) -> String {
    if case.name.is_empty() {
        dc.path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    } else {
        case.name.clone()
    }
}

/// 加载项目上下文：定位 root、加载 requestbaby.json 与 env 文件
pub fn load_project_ctx(start: &Path, env_name: &str) -> Result<ProjectCtx> {
    let root = discover::find_project_root(start);
    let project = match &root {
        Some(r) => discover::project_file(r).and_then(|p| load_strict::<Project>(&p).ok()).unwrap_or_default(),
        None => Project::default(),
    };
    let env_vars = match &root {
        Some(r) => {
            let env_path = r.join("envs").join(format!("{env_name}.json"));
            if env_path.exists() {
                let f = load_strict::<crate::config::EnvFile>(&env_path)
                    .with_context(|| format!("环境文件 {env_name} 加载失败"))?;
                f.vars.plain()
            } else {
                BTreeMap::new()
            }
        }
        None => BTreeMap::new(),
    };
    let secret_values = match &root {
        Some(r) => {
            let mut s = load_strict::<Project>(&r.join(discover::PROJECT_FILE))
                .map(|p| p.vars.secret_values())
                .unwrap_or_default();
            let env_path = r.join("envs").join(format!("{env_name}.json"));
            if let Ok(f) = load_strict::<crate::config::EnvFile>(&env_path) {
                s.extend(f.vars.secret_values());
            }
            s
        }
        None => vec![],
    };
    Ok(ProjectCtx {
        project,
        env_vars,
        secret_values,
    })
}
