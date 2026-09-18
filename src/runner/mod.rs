pub mod discover;

use crate::config::{load_strict, Collection, Project, TestCase};
use crate::engine::{CurlTransport, ResolvedRequest, Response, Transport};
use crate::script::{QuickJsHost, ReqData, RespData, ScriptHost, TestOutcome};
use crate::vars::Vars;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::Path;

/// 运行选项
pub struct RunOptions {
    pub tags: Vec<String>,
    pub name_filter: String,
    pub fail_fast: bool,
    pub dry_run: bool,
    pub verbose: bool,
}

/// 单用例执行结果
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
}

impl CaseResult {
    pub fn passed(&self) -> bool {
        !self.skipped && self.error.is_none() && self.tests.iter().all(|t| t.pass)
    }
}

/// 项目上下文（一次 run 内共享）
pub struct ProjectCtx {
    pub project: Project,
    pub env_vars: BTreeMap<String, String>,
}

/// 编排器：discover → filter → execute（顺序）→ results
pub struct Runner {
    ctx: ProjectCtx,
    transport: CurlTransport,
    script: QuickJsHost,
    runtime_vars: BTreeMap<String, String>,
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
        v.push_layer(self.ctx.project.vars.0.clone());
        v.push_layer(self.ctx.env_vars.clone());
        for c in chain {
            v.push_layer(c.vars.0.clone());
        }
        v.push_layer(self.runtime_vars.clone());
        v
    }

    fn matches_filter(tags: &[String], name: &str, opts: &RunOptions) -> bool {
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
        if !opts.name_filter.is_empty()
            && !name
                .to_lowercase()
                .contains(&opts.name_filter.to_lowercase())
        {
            return false;
        }
        true
    }

    /// 执行全部发现用例（按 rel 路径序）
    pub fn run(
        &mut self,
        cases: &[discover::DiscoveredCase],
        opts: &RunOptions,
        mut sink: impl FnMut(&CaseResult),
    ) -> Result<Vec<CaseResult>> {
        let mut results = vec![];
        for dc in cases {
            let case = match load_strict::<TestCase>(&dc.path) {
                Ok(c) => c,
                Err(e) => {
                    let r = CaseResult {
                        rel: dc.rel.clone(),
                        name: dc
                            .path
                            .file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                        tags: vec![],
                        method: "-".into(),
                        url: "-".into(),
                        status: 0,
                        duration_ms: 0,
                        skipped: false,
                        error: Some(format!("{e:#}")),
                        tests: vec![],
                        curl_cmd: None,
                    };
                    sink(&r);
                    results.push(r);
                    if opts.fail_fast {
                        return Ok(results);
                    }
                    continue;
                }
            };

            // 合并集合级 tags，计算展示名
            let mut all_tags: Vec<String> = dc.chain.iter().flat_map(|c| c.tags.clone()).collect();
            all_tags.extend(case.tags.iter().cloned());
            let name = if case.name.is_empty() {
                dc.path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            } else {
                case.name.clone()
            };

            if !Self::matches_filter(&all_tags, &name, opts) {
                continue;
            }

            let r = self.execute_case(dc, &case, &all_tags, &name, opts);
            let stop = opts.fail_fast && !r.passed();
            sink(&r);
            results.push(r);
            if stop {
                return Ok(results);
            }
        }
        Ok(results)
    }

    fn execute_case(
        &mut self,
        dc: &discover::DiscoveredCase,
        case: &TestCase,
        tags: &[String],
        name: &str,
        opts: &RunOptions,
    ) -> CaseResult {
        let vars = self.vars_for(&dc.chain);
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
        };
        if case.skip {
            return result;
        }

        // ---- 构建请求（变量替换）----
        let timeout = case.request.timeout_ms.unwrap_or(self.ctx.project.timeout_ms);
        let mut headers: Vec<(String, String)> = case
            .request
            .headers
            .iter()
            .map(|(k, v)| (k.clone(), vars.render(v)))
            .collect();
        let body_str = case.request.body.as_ref().map(|b| vars.render(&b.render()));
        if body_str.is_some()
            && case.request.body.as_ref().map(|b| b.is_json()).unwrap_or(false)
            && !headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("content-type"))
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
                        eprintln!("  [log] {}", out.logs.join(" | "));
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

        // ---- 编译 curl ----
        let inv = match self.transport.compile(&req) {
            Ok(i) => i,
            Err(e) => {
                result.error = Some(format!("编译 curl 失败: {e:#}"));
                return result;
            }
        };
        result.curl_cmd = Some(inv.argv.join(" "));

        if opts.dry_run {
            return result;
        }

        // ---- 执行 ----
        let resp: Response = match self.transport.run(&inv) {
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
                        eprintln!("  [log] {}", out.logs.join(" | "));
                    }
                }
                Err(e) => {
                    result.error = Some(format!("post 脚本宿主错误: {e:#}"));
                }
            }
        }

        // ---- tests ----
        if !case.tests.is_empty() {
            let env_now = self.vars_for(&dc.chain).env_snapshot();
            match self
                .script
                .eval_tests(&resp_data, &self.runtime_vars, &env_now, &case.tests)
            {
                Ok(ts) => result.tests = ts,
                Err(e) => result.error = Some(format!("tests 宿主错误: {e:#}")),
            }
        }
        result
    }
}

/// 加载项目上下文：定位 root、加载 apicase.json 与 env 文件
pub fn load_project_ctx(start: &Path, env_name: &str) -> Result<ProjectCtx> {
    let root = discover::find_project_root(start);
    let project = match &root {
        Some(r) => load_strict::<Project>(&r.join("apicase.json")).unwrap_or_default(),
        None => Project::default(),
    };
    let env_vars = match &root {
        Some(r) => {
            let env_path = r.join("envs").join(format!("{env_name}.json"));
            if env_path.exists() {
                let f = load_strict::<crate::config::EnvFile>(&env_path)
                    .with_context(|| format!("环境文件 {env_name} 加载失败"))?;
                f.vars.0
            } else {
                BTreeMap::new()
            }
        }
        None => BTreeMap::new(),
    };
    Ok(ProjectCtx {
        project,
        env_vars,
    })
}
