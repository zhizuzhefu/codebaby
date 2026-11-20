pub mod discover;
mod event;
pub mod ui;

pub use event::{CancelToken, EventHook, RunEvent};
pub use ui::ArtifactsDir;

use crate::config::case::append_query;
use crate::config::{enabled_kv, load_strict, BodySpec, Collection, MultipartValue, Project, Protocol, TestCase, WsAction};
use crate::cookies::CookieJar;
use crate::engine::{CurlTransport, FormPart, PartValue, ResolvedRequest, Response, Transport};
use crate::script::{QuickJsHost, ReqData, RespData, ScriptHost, TestOutcome};
use crate::vars::{VarVal, Vars};
use anyhow::Result;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

/// 运行选项
#[derive(Clone, Default)]
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
    /// 协作式取消（Desktop 停止按钮 / Ctrl-C）
    pub cancel: Option<CancelToken>,
    /// 界面用例的证据目录（截图 / 失败现场）；None 时放到系统临时目录
    pub artifacts: Option<ArtifactsDir>,
    /// Web 用例显示浏览器窗口（--headed）
    pub headed: bool,
}

/// 响应快照（进报告与运行历史：失败用例保留 4000 字符辅助 AI 自修复，通过的保留 2000 字符供回看）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResponseSnapshot {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub duration_ms: u64,
    /// body 被截断（完整响应需重新发送）
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Protocol::is_http")]
    pub protocol: Protocol,
    /// 消息时间线（WebSocket / gRPC；条数与每条长度有上限）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frames: Vec<crate::engine::Frame>,
    /// 界面用例的证据：每一步的记录与截图（截图文件在证据目录里）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<crate::engine::ui::UiEvidence>,
}

/// 状态文字：HTTP / WebSocket 显示数字，gRPC 显示状态名，界面用例执行完为「完成」（Web 有页面状态码时显示数字）；没有响应为 -
pub fn status_text(protocol: Protocol, status: u16, responded: bool) -> String {
    match (protocol, responded) {
        (_, false) => "-".into(),
        (Protocol::Grpc, true) => crate::config::protocols::grpc_code_name(status),
        (Protocol::Web, true) if status > 0 => status.to_string(),
        (Protocol::Web | Protocol::Android | Protocol::Desktop, true) => "完成".into(),
        _ => status.to_string(),
    }
}

/// 状态是否异常
pub fn status_is_bad(protocol: Protocol, status: u16, responded: bool) -> bool {
    !responded
        || match protocol {
            Protocol::Http | Protocol::Web => status >= 400,
            Protocol::Ws => status != 101,
            Protocol::Grpc => status != 0,
            Protocol::Android | Protocol::Desktop => false,
        }
}

/// 一次交换的结果：统一成 RespData，后面的 capture / post / expect / tests 与协议无关
struct Exchanged {
    resp: RespData,
    /// 交换过程中产生的断言（WebSocket 步骤里的 receive / waitFor / 步骤 capture）
    outcomes: Vec<TestOutcome>,
}

/// 响应是否为二进制（图片 / 文件下载等）：按 Content-Type，或内容不是合法 UTF-8
pub fn is_binary(headers: &[(String, String)], body: &[u8]) -> bool {
    let ct = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-type")).map(|(_, v)| v.to_ascii_lowercase()).unwrap_or_default();
    let binary_ct = ["image/", "audio/", "video/", "application/octet-stream", "application/pdf", "application/zip", "application/gzip", "font/"]
        .iter()
        .any(|p| ct.starts_with(p))
        && !ct.starts_with("image/svg");
    binary_ct || std::str::from_utf8(body).is_err()
}

/// 单用例执行结果
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CaseResult {
    pub rel: String,
    pub name: String,
    pub tags: Vec<String>,
    /// HTTP 方法；gRPC 为 GRPC，WebSocket 为 WS
    pub method: String,
    /// HTTP / WebSocket 地址；gRPC 为「地址/包.服务/方法」
    pub url: String,
    #[serde(default, skip_serializing_if = "Protocol::is_http")]
    pub protocol: Protocol,
    /// HTTP 状态码（WebSocket 为握手状态，成功 101）；gRPC 为 gRPC 状态码（0 = OK）
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
    /// 二进制响应的原始字节（send 模式；图片预览 / 另存为）
    #[serde(skip)]
    pub raw_body: Option<std::sync::Arc<Vec<u8>>>,
    /// 脚本 console.log 输出（已脱敏）
    #[serde(default)]
    pub logs: Vec<String>,
    /// 用例结束的时刻（Unix 毫秒）；多端混合运行的时间线据此排序。0 = 未记录
    #[serde(default)]
    pub finished_at_ms: u64,
}

impl CaseResult {
    /// 记下结束时刻（只记一次：并发工作线程里已记过的不覆盖）
    pub(crate) fn stamped(mut self) -> Self {
        if self.finished_at_ms == 0 {
            self.finished_at_ms = crate::history::now_ms();
        }
        self
    }

    pub fn passed(&self) -> bool {
        !self.skipped && self.error.is_none() && self.tests.iter().all(|t| t.pass)
    }

    /// 拿到了响应（gRPC 的 OK 是 0，不能用 status 是否为 0 判断）
    pub fn responded(&self) -> bool {
        self.response_snapshot.is_some() || self.full_response.is_some() || (matches!(self.protocol, Protocol::Http | Protocol::Ws) && self.status > 0)
    }

    /// 状态列文字：200 / 101 / OK / NOT_FOUND；没有响应时为 -
    pub fn status_text(&self) -> String {
        status_text(self.protocol, self.status, self.responded())
    }

    /// 状态是否算异常（标红）：没有响应、HTTP ≥ 400、WebSocket 握手没成功、gRPC 非 OK
    pub fn status_is_bad(&self) -> bool {
        status_is_bad(self.protocol, self.status, self.responded())
    }

    /// 错误/占位构造器（统一 5 处样板）
    pub fn placeholder(rel: &str, name: &str, error: Option<String>) -> Self {
        Self {
            rel: rel.to_string(),
            name: name.to_string(),
            tags: vec![],
            method: "-".into(),
            url: "-".into(),
            protocol: Protocol::Http,
            status: 0,
            duration_ms: 0,
            skipped: false,
            error,
            tests: vec![],
            curl_cmd: None,
            body_preview: None,
            response_snapshot: None,
            full_response: None,
            raw_body: None,
            logs: vec![],
            finished_at_ms: 0,
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
    /// 代理规则（requestbaby.json 的 proxy + REQUESTBABY_PROXY 覆盖）
    pub proxy: crate::engine::proxy::ProxyPolicy,
}

pub use crate::cookies::{OriginStorage, WebStorage};

/// 一个用户的登录态：Cookie + 网页存储。切换用户 = 换一份
#[derive(Debug, Clone, Default)]
pub struct UserSession {
    pub cookies: CookieJar,
    pub storage: WebStorage,
}

/// 跨请求的会话状态：运行时捕获的变量 + 默认用户的登录态（Cookie、网页存储）+ 其他用户各自的登录态。
/// 一次运行内自动共享；Desktop 在本次会话内跨「发送」保持（[`Runner::seed`] / [`Runner::state`]）
#[derive(Debug, Clone, Default)]
pub struct SessionState {
    pub vars: BTreeMap<String, VarVal>,
    pub cookies: CookieJar,
    pub storage: WebStorage,
    /// 命名用户（流程步骤的 `as`）的登录态，不含默认用户
    pub users: BTreeMap<String, UserSession>,
}

impl SessionState {
    pub fn is_empty(&self) -> bool {
        self.vars.is_empty() && self.cookies.is_empty() && self.storage.is_empty() && self.users.is_empty()
    }

    /// 后来者覆盖：同名变量 / 同一 Cookie / 同一站点的网页存储以 other 为准
    pub fn merge(&mut self, other: &SessionState) {
        self.vars.extend(other.vars.iter().map(|(k, v)| (k.clone(), v.clone())));
        self.cookies.merge(&other.cookies);
        self.storage.extend(other.storage.iter().map(|(k, v)| (k.clone(), v.clone())));
        for (name, u) in &other.users {
            let mine = self.users.entry(name.clone()).or_default();
            mine.cookies.merge(&u.cookies);
            mine.storage.extend(u.storage.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
    }
}

/// 编排器：discover → filter → execute（顺序/并行）→ results
#[derive(Clone)]
pub struct Runner {
    ctx: ProjectCtx,
    /// 执行引擎（libcurl binding）
    transport: CurlTransport,
    script: QuickJsHost,
    runtime_vars: BTreeMap<String, VarVal>,
    /// 当前用户的登录态：Cookie + 网页存储（HTTP / Web 用例共用）
    cookies: CookieJar,
    storage: WebStorage,
    /// 暂时不用的用户的登录态（含被切走的默认用户，键为空字符串）
    parked_users: BTreeMap<String, UserSession>,
    /// 当前用户（None = 默认用户）
    user: Option<String>,
    /// 运行期事件回调（caseStarted / log / caseFinished / notice）
    hook: Option<EventHook>,
    /// 非用例级提示（teardown 失败等），由上层渲染
    notices: Vec<String>,
    /// 界面用例共享的浏览器 / 已安装的 APK（一次运行内复用）
    ui: std::sync::Arc<std::sync::Mutex<ui::UiResources>>,
}

impl Runner {
    pub fn new(ctx: ProjectCtx) -> Self {
        Self {
            script: QuickJsHost::with_proxy(ctx.proxy.clone()),
            ctx,
            transport: CurlTransport::new(),
            runtime_vars: BTreeMap::new(),
            cookies: CookieJar::default(),
            storage: WebStorage::new(),
            parked_users: BTreeMap::new(),
            user: None,
            hook: None,
            notices: vec![],
            ui: Default::default(),
        }
    }

    /// 以已有会话状态开始（Desktop 单发：带上之前捕获的 token / Cookie）
    pub fn seed(&mut self, state: SessionState) {
        self.runtime_vars = state.vars;
        self.cookies = state.cookies;
        self.storage = state.storage;
        self.parked_users = state.users;
        self.user = None;
    }

    /// 当前会话状态（运行结束后回传给 Desktop 会话）：以默认用户为主，不管此刻正以谁的身份在跑
    pub fn state(&self) -> SessionState {
        let mut users = self.parked_users.clone();
        let current = UserSession { cookies: self.cookies.clone(), storage: self.storage.clone() };
        let default = match &self.user {
            None => current,
            Some(name) => {
                users.insert(name.clone(), current);
                users.remove("").unwrap_or_default()
            }
        };
        SessionState { vars: self.runtime_vars.clone(), cookies: default.cookies, storage: default.storage, users }
    }

    /// 切换用户：每个用户有各自的 Cookie 与网页存储，捕获的变量仍然共享。None = 默认用户；
    /// 第一次用到的用户从空白登录态开始
    pub fn switch_user(&mut self, name: Option<&str>) {
        let want = name.filter(|n| !n.is_empty());
        if want == self.user.as_deref() {
            return;
        }
        let parked = UserSession { cookies: std::mem::take(&mut self.cookies), storage: std::mem::take(&mut self.storage) };
        self.parked_users.insert(self.user.clone().unwrap_or_default(), parked);
        let next = self.parked_users.remove(want.unwrap_or("")).unwrap_or_default();
        self.cookies = next.cookies;
        self.storage = next.storage;
        self.user = want.map(String::from);
    }

    /// 安装事件回调（Desktop 进度 / CLI `--json-stream`）
    pub fn set_hook(&mut self, hook: EventHook) {
        self.hook = Some(hook);
    }

    /// 取走运行期提示（teardown 失败等）
    pub fn take_notices(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notices)
    }

    fn emit(&self, ev: RunEvent) {
        if let Some(h) = &self.hook {
            h(&ev);
        }
    }

    fn notice(&mut self, message: String) {
        self.emit(RunEvent::Notice { message: message.clone() });
        self.notices.push(message);
    }

    fn cancelled(opts: &RunOptions) -> bool {
        opts.cancel.as_ref().map(|c| c.is_cancelled()).unwrap_or(false)
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
        let hook = self.hook.clone();
        let mut sink = move |r: &CaseResult| {
            if let Some(h) = &hook {
                h(&RunEvent::CaseFinished { result: crate::report::CaseReport::from_result(r, true) });
            }
            sink(r);
        };
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
                for n in c.var_writes() {
                    if !written.contains(&n) {
                        written.push(n);
                    }
                }
            }
        }
        let consumes_written = |c: &TestCase| -> bool {
            let texts = c.referencing_texts();
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
            // 写变量的（脚本 / capture）与读这些变量的都串行，保证捕获先于引用
            // 界面用例同样串行：共用一个浏览器 / 一台设备，截图证据按顺序可读
            let depends = !case.pre.is_empty() || !case.post.is_empty() || !case.capture.is_empty() || case.cases.is_some() || consumes_written(&case) || case.protocol().is_ui();
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
        // 阶段2：独立用例由 width 个工作线程领取执行（不按批等待：慢用例不拖住其他线程）；
        // 结果按原顺序交给 sink。跳过 setup/teardown；fail_fast 时不再领取新用例
        let jobs: Vec<discover::DiscoveredCase> = parallel.into_iter().cloned().collect();
        let next = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let jobs = std::sync::Arc::new(jobs);
        let (tx, rx) = std::sync::mpsc::channel::<(usize, Vec<CaseResult>)>();
        let mut workers = vec![];
        for _ in 0..width.min(jobs.len()) {
            let (jobs, next, stop, tx) = (jobs.clone(), next.clone(), stop.clone(), tx.clone());
            let mut runner = self.clone();
            let o = RunOptions {
                verbose: opts.verbose,
                output_dir: opts.output_dir.clone(),
                cancel: opts.cancel.clone(),
                artifacts: opts.artifacts.clone(),
                headed: opts.headed,
                ..Default::default()
            };
            let fail_fast = opts.fail_fast;
            workers.push(std::thread::spawn(move || loop {
                if stop.load(std::sync::atomic::Ordering::Relaxed) || Self::cancelled(&o) {
                    break;
                }
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(dc) = jobs.get(i) else { break };
                let mut out: Vec<CaseResult> = vec![];
                let _ = runner.run_serial(std::slice::from_ref(dc), &o, &mut |x| out.push(x.clone()));
                if fail_fast && out.iter().any(|r| !r.passed()) {
                    stop.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                if tx.send((i, out)).is_err() {
                    break;
                }
            }));
        }
        drop(tx);
        // 按原顺序吐出：先到的乱序结果暂存，等前面的到齐
        let mut pending: BTreeMap<usize, Vec<CaseResult>> = BTreeMap::new();
        let mut emit = 0usize;
        for (i, rs) in rx {
            pending.insert(i, rs);
            while let Some(rs) = pending.remove(&emit) {
                for r in rs {
                    let r = r.stamped();
                    sink(&r);
                    results.push(r);
                }
                emit += 1;
            }
        }
        // fail_fast / 取消时序号可能不连续：剩下的按序补上
        for (_, rs) in pending {
            for r in rs {
                let r = r.stamped();
                sink(&r);
                results.push(r);
            }
        }
        for w in workers {
            let _ = w.join();
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
            if Self::cancelled(opts) {
                break;
            }
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
                            let r = r.stamped();
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
                    let r = r.stamped();
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
            let rows: Vec<Option<crate::config::DataRow>> = match &case.cases {
                None => vec![None],
                Some(spec) => {
                    let base = dc.path.parent().unwrap_or(std::path::Path::new("."));
                    match spec.resolve(base) {
                        Ok(rs) if !rs.is_empty() => rs.into_iter().map(Some).collect(),
                        Ok(_) => vec![None],
                        Err(e) => {
                            let r = self.error_result(dc, &name, &all_tags, format!("数据文件加载失败: {e}"));
                            let r = r.stamped();
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
                if Self::cancelled(opts) {
                    stop = true;
                    break;
                }
                let inst_name = match (&case.cases, row) {
                    (Some(_), Some(r)) => {
                        let label = r.get("label").map(|v| v.to_render()).unwrap_or_else(|| format!("#{}", i));
                        format!("{name} [{label}]")
                    }
                    _ => name.clone(),
                };
                let r = self.execute_case(dc, &case, &all_tags, &inst_name, row.as_ref(), opts);
                let bad = !r.passed();
                let r = r.stamped();
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
                    self.notice(format!("[teardown] {} {}", lvl.display(), r.error.as_deref().unwrap_or("")));
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
        // 失败结果：文件 = 该 collection.json（相对项目根，点开即是集合配置），方法列显示 setup / teardown
        let rel = discover::find_project_root(dir)
            .and_then(|root| cfg_path.strip_prefix(&root).ok().map(|p| p.to_string_lossy().replace('\\', "/")))
            .unwrap_or_else(|| cfg_path.display().to_string());
        let failed = |error: String| {
            let mut r = CaseResult::placeholder(&rel, &format!("集合 {kind}：{display_name}"), Some(error));
            r.method = kind.to_string();
            r.url = String::new();
            r
        };
        match self.script.run_scoped(&self.runtime_vars, &env, script) {
            Ok(out) => {
                if let Some(e) = out.error {
                    return Some(failed(format!("{kind} 脚本失败: {e}")));
                }
                self.runtime_vars.extend(out.vars);
            }
            Err(e) => return Some(failed(format!("{kind} 宿主错误: {e:#}"))),
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
            protocol: Protocol::Http,
            status: 0,
            duration_ms: 0,
            skipped: false,
            error: Some(msg),
            tests: vec![],
            curl_cmd: None,
            body_preview: None,
            response_snapshot: None,
            full_response: None,
            raw_body: None,
            logs: vec![],
            finished_at_ms: 0,
        }
    }

    fn execute_case(
        &mut self,
        dc: &discover::DiscoveredCase,
        case: &TestCase,
        tags: &[String],
        name: &str,
        row: Option<&crate::config::DataRow>,
        opts: &RunOptions,
    ) -> CaseResult {
        let mut vars = self.vars_for(&dc.chain);
        // 数据驱动行变量：优先级最高
        if let Some(row) = row {
            vars.push_layer(row.clone());
        }
        let mut result = CaseResult {
            rel: dc.rel.clone(),
            name: name.to_string(),
            tags: tags.to_vec(),
            method: "-".into(),
            url: "-".into(),
            protocol: case.protocol(),
            status: 0,
            duration_ms: 0,
            skipped: case.skip,
            error: None,
            tests: vec![],
            curl_cmd: None,
            body_preview: None,
            response_snapshot: None,
            full_response: None,
            raw_body: None,
            logs: vec![],
            finished_at_ms: 0,
        };
        if case.skip {
            return result;
        }
        self.emit(RunEvent::CaseStarted { file: dc.rel.clone(), name: name.to_string() });

        if let Some(e) = case.spec_error() {
            result.error = Some(e);
            return result;
        }
        let protocol = case.protocol();
        result.protocol = protocol;
        let env_view = vars.env_snapshot();
        let exchanged = match protocol {
            Protocol::Http => self.exchange_http(dc, case, &vars, &env_view, &mut result, opts),
            Protocol::Grpc => self.exchange_grpc(dc, case, &vars, &env_view, &mut result, opts),
            Protocol::Ws => self.exchange_ws(dc, case, row, &vars, &env_view, &mut result, opts),
            Protocol::Web => self.exchange_web(dc, case, row, &vars, &env_view, &mut result, opts),
            Protocol::Android => self.exchange_android(dc, case, row, &vars, &env_view, &mut result, opts),
            Protocol::Desktop => self.exchange_desktop(dc, case, row, &vars, &mut result, opts),
        };
        let Some(Exchanged { resp: resp_data, outcomes: step_outcomes }) = exchanged else { return result };
        result.status = resp_data.status;
        result.duration_ms = resp_data.time_ms;

        let actual = crate::expect::Actual { status: resp_data.status, headers: &resp_data.headers, body: &resp_data.body, protocol };
        // ---- capture：声明式捕获，先于 post 脚本（脚本里可直接用捕获到的变量）----
        let mut outcomes: Vec<TestOutcome> = step_outcomes;
        if !case.capture.is_empty() {
            let captured = crate::expect::capture(&case.capture, &actual);
            self.runtime_vars.extend(captured.vars);
            outcomes.extend(captured.failures);
        }

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
                    self.collect_logs(&mut result, &dc.rel, &out.logs);
                }
                Err(e) => {
                    result.error = Some(format!("post 脚本宿主错误: {e:#}"));
                }
            }
        }

        // ---- 断言：expect（声明式，含旧写法 expectStatus）→ tests（JS）----
        let mut env_vars = self.vars_for(&dc.chain);
        if let Some(row) = row {
            env_vars.push_layer(row.clone());
        }
        let mut expect = case.expect.clone().unwrap_or_default();
        if expect.status.is_none() {
            expect.status = case.expect_status.clone();
        }
        if !expect.is_empty() {
            let mut with_runtime = env_vars.clone();
            with_runtime.push_layer(self.runtime_vars.clone());
            outcomes.extend(crate::expect::evaluate(&expect, &actual, &with_runtime));
        }
        if !case.tests.is_empty() {
            let env_now = env_vars.env_snapshot();
            match self
                .script
                .eval_tests(&resp_data, &self.runtime_vars, &env_now, &case.tests)
            {
                Ok(ts) => outcomes.extend(ts),
                Err(e) => result.error = Some(format!("tests 宿主错误: {e:#}")),
            }
        }
        result.tests = outcomes;

        // ---- 收尾：send 快照 / 失败快照 / 响应落盘（-o）/ 输出脱敏 ----
        self.finalize_result(&mut result, dc, &resp_data, opts);
        result
    }

    /// HTTP：组装请求 → pre 脚本 → 残留占位符检查 → Cookie → 编译 curl（dry-run 到此）→ 发送
    fn exchange_http(
        &mut self,
        dc: &discover::DiscoveredCase,
        case: &TestCase,
        vars: &Vars,
        env_view: &BTreeMap<String, VarVal>,
        result: &mut CaseResult,
        opts: &RunOptions,
    ) -> Option<Exchanged> {
        // ---- 构建请求（变量替换 + headers 继承合并）----
        let spec = case.request.as_ref().expect("HTTP 用例");
        result.method = spec.method.clone();
        result.url = spec.url.clone();
        let timeout = spec.timeout_ms.unwrap_or(self.ctx.project.timeout_ms);
        // 集合链 headers（浅→深）→ 用例 headers 覆盖（key 小写归一防重复发送）
        let mut merged_headers: BTreeMap<String, (String, String)> = BTreeMap::new();
        for c in &dc.chain {
            for (k, v) in enabled_kv(&c.headers) {
                merged_headers.insert(k.to_lowercase(), (k.clone(), v.to_string()));
            }
        }
        for (k, v) in enabled_kv(&spec.headers) {
            merged_headers.insert(k.to_lowercase(), (k.clone(), v.to_string()));
        }
        let mut headers: Vec<(String, String)> = merged_headers
            .values()
            .map(|(k, v)| (k.clone(), vars.render(v)))
            .collect();
        let body = spec.body.as_ref();
        let body_str = body.and_then(|b| b.render_with(|t| vars.render(t)));
        if let Some(ct) = body.and_then(|b| b.default_content_type()) {
            if !merged_headers.contains_key("content-type") {
                headers.push(("Content-Type".into(), ct.into()));
            }
        }
        let case_dir = dc.path.parent().unwrap_or(std::path::Path::new("."));
        let multipart = match body {
            Some(BodySpec::Multipart { multipart }) => multipart
                .iter()
                .map(|(k, v)| FormPart {
                    name: vars.render(k),
                    value: match v {
                        MultipartValue::Text(t) => PartValue::Text(vars.render(t)),
                        MultipartValue::File { file, filename, content_type } => PartValue::File {
                            path: case_dir.join(vars.render(file)),
                            filename: filename.as_ref().map(|f| vars.render(f)),
                            content_type: content_type.clone(),
                        },
                    },
                })
                .collect(),
            _ => vec![],
        };
        let query: Vec<(String, String)> =
            enabled_kv(&spec.query).map(|(k, v)| (vars.render(k), vars.render(v))).collect();
        let mut req = ResolvedRequest {
            method: spec.method.clone(),
            url: append_query(&vars.render(&spec.url), &query),
            headers,
            body: body_str,
            multipart,
            timeout_ms: timeout,
            retry: spec.retry.unwrap_or(0),
            insecure: spec.insecure.unwrap_or(false),
            follow_redirects: spec.follow_redirects.unwrap_or(false),
            proxy: self.ctx.proxy.clone(),
        };

        // ---- pre 脚本 ----
        if !case.pre.is_empty() {
            let req_data = ReqData {
                method: req.method.clone(),
                url: req.url.clone(),
                headers: req.headers.iter().cloned().collect(),
                body: req.body.clone(),
            };
            if let Some(rd) = self.run_pre_script(dc, case, env_view, req_data, result)? {
                req.method = rd.method;
                req.url = rd.url;
                req.headers = rd.headers.into_iter().collect();
                req.body = rd.body;
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
            for part in &mut req.multipart {
                match &mut part.value {
                    PartValue::Text(t) => *t = vars2.render(t),
                    PartValue::File { path, .. } => *path = vars2.render(&path.to_string_lossy()).into(),
                }
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
            for part in &req.multipart {
                texts.push(match &part.value {
                    PartValue::Text(t) => t.clone(),
                    PartValue::File { path, .. } => path.to_string_lossy().into_owned(),
                });
            }
            if let Some(e) = leftover_error(&texts) {
                result.error = Some(e);
                return None;
            }
        }

        // ---- Cookie：自动带上之前响应设置的 Cookie（用例显式写了 Cookie 头则以用例为准）----
        if self.ctx.project.cookies && !req.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("cookie")) {
            if let Some(cookie) = self.cookies.header_for(&req.url, crate::history::now_ms()) {
                req.headers.push(("Cookie".into(), cookie));
            }
        }

        // ---- 编译 curl 命令展示（dry-run / -v）----
        let inv = match self.transport.compile(&req) {
            Ok(i) => i,
            Err(e) => {
                result.error = Some(format!("编译 curl 失败: {e:#}"));
                return None;
            }
        };
        result.curl_cmd = Some(inv.shell());

        if opts.dry_run {
            if let Some(content) = &inv.body {
                let mut preview: String = content.chars().take(800).collect();
                if content.len() > 800 {
                    preview.push('…');
                }
                result.body_preview = Some(preview);
            }
            self.redact_result(result);
            return None;
        }

        // ---- 执行 ----
        let resp: Response = match self.transport.execute(&req) {
            Ok(r) => r,
            Err(e) => {
                result.error = Some(format!("{e:#}"));
                return None;
            }
        };
        result.status = resp.status;
        result.duration_ms = resp.time_ms;
        if self.ctx.project.cookies && resp.status > 0 {
            let set_cookies = resp.headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie")).map(|(_, v)| v.as_str());
            self.cookies.store(&req.url, set_cookies, crate::history::now_ms());
        }

        if let Some(e) = &resp.transport_error {
            result.error = Some(format!("传输错误: {e}"));
            self.redact_result(result);
            return None;
        }

        let binary = is_binary(&resp.headers, &resp.body);
        if opts.send_mode && binary {
            result.raw_body = Some(std::sync::Arc::new(resp.body.clone()));
        }
        let resp_data = RespData::http(resp.status, resp.time_ms, resp.size, resp.headers.clone(), resp.body_text());
        Some(Exchanged { resp: resp_data, outcomes: vec![] })
    }

    /// pre 脚本：Ok(Some) = 脚本改写后的请求；Err(()) = 失败（错误已写入 result）
    fn run_pre_script(
        &mut self,
        dc: &discover::DiscoveredCase,
        case: &TestCase,
        env_view: &BTreeMap<String, VarVal>,
        req: ReqData,
        result: &mut CaseResult,
    ) -> Option<Option<ReqData>> {
        match self.script.run_pre(req, &self.runtime_vars, env_view, &case.pre) {
            Ok(out) => {
                if let Some(e) = out.error {
                    result.error = Some(format!("pre 脚本失败: {e}"));
                    return None;
                }
                self.runtime_vars.extend(out.vars);
                self.collect_logs(result, &dc.rel, &out.logs);
                Some(out.req)
            }
            Err(e) => {
                result.error = Some(format!("pre 脚本宿主错误: {e:#}"));
                None
            }
        }
    }

    /// 集合链 + 用例自己的请求头 / 元数据（浅 → 深，用例最后；键名不分大小写去重），值已替换变量
    fn merged_headers(&self, dc: &discover::DiscoveredCase, own: &BTreeMap<String, crate::config::KvValue>, vars: &Vars, skip: &[&str]) -> Vec<(String, String)> {
        let mut merged: BTreeMap<String, (String, String)> = BTreeMap::new();
        for c in &dc.chain {
            for (k, v) in enabled_kv(&c.headers) {
                merged.insert(k.to_lowercase(), (k.clone(), v.to_string()));
            }
        }
        for (k, v) in enabled_kv(own) {
            merged.insert(k.to_lowercase(), (k.clone(), v.to_string()));
        }
        merged.into_iter().filter(|(lk, _)| !skip.iter().any(|s| lk == s || (s.ends_with('*') && lk.starts_with(s.trim_end_matches('*'))))).map(|(_, (k, v))| (k, vars.render(&v))).collect()
    }

    /// gRPC：组装调用 → pre 脚本 → 残留占位符检查 → 等价 grpcurl 命令（dry-run 到此，配置了 .proto 时离线校验方法与消息）→ 调用
    fn exchange_grpc(
        &mut self,
        dc: &discover::DiscoveredCase,
        case: &TestCase,
        vars: &Vars,
        env_view: &BTreeMap<String, VarVal>,
        result: &mut CaseResult,
        opts: &RunOptions,
    ) -> Option<Exchanged> {
        use crate::engine::grpc;
        let spec = case.grpc.as_ref().expect("gRPC 用例");
        result.method = "GRPC".into();
        // 集合的公共请求头同样作为元数据发送（HTTP 专用的头跳过；gRPC 元数据名一律小写）
        const RESERVED: [&str; 9] = ["content-type", "content-length", "user-agent", "host", "connection", "te", "accept", "accept-encoding", "grpc-*"];
        let mut metadata: Vec<(String, String)> = self.merged_headers(dc, &spec.metadata, vars, &RESERVED).into_iter().map(|(k, v)| (k.to_lowercase(), v)).collect();
        let mut target = vars.render(&spec.target);
        let mut method = vars.render(&spec.method);
        let streaming_input = spec.messages.is_some();
        let mut messages: Vec<String> = spec.outgoing().iter().map(|m| vars.render(&serde_json::to_string(m).unwrap_or_default())).collect();

        if !case.pre.is_empty() {
            // 脚本里：req.url = 地址，req.method = 包.服务/方法，req.headers = 元数据，req.body = 消息 JSON（messages 时为数组）
            let body = if streaming_input { format!("[{}]", messages.join(",")) } else { messages.first().cloned().unwrap_or_default() };
            let req = ReqData { method: method.clone(), url: target.clone(), headers: metadata.iter().cloned().collect(), body: Some(body) };
            if let Some(rd) = self.run_pre_script(dc, case, env_view, req, result)? {
                target = rd.url;
                method = rd.method;
                metadata = rd.headers.into_iter().collect();
                let body = rd.body.unwrap_or_default();
                messages = match serde_json::from_str::<serde_json::Value>(&body) {
                    Ok(serde_json::Value::Array(items)) if streaming_input => items.iter().map(|m| m.to_string()).collect(),
                    _ => vec![body],
                };
            }
            let vars2 = self.vars_for(&dc.chain);
            target = vars2.render(&target);
            method = vars2.render(&method);
            metadata = metadata.into_iter().map(|(k, v)| (k, vars2.render(&v))).collect();
            messages = messages.iter().map(|m| vars2.render(m)).collect();
        }
        result.url = format!("{}/{}", target.trim_end_matches('/'), method);

        if !opts.dry_run {
            let mut texts = vec![target.clone(), method.clone()];
            texts.extend(metadata.iter().map(|(_, v)| v.clone()));
            texts.extend(messages.iter().cloned());
            if let Some(e) = leftover_error(&texts) {
                result.error = Some(e);
                return None;
            }
        }
        let root = discover::find_project_root(&dc.path);
        let source = match grpc::Source::from_config(root.as_deref().unwrap_or(Path::new(".")), &self.ctx.project.grpc) {
            Ok(s) => s,
            Err(e) => {
                result.error = Some(format!("{e:#}"));
                return None;
            }
        };
        let call = grpc::GrpcCall {
            target,
            method,
            messages,
            metadata,
            timeout_ms: spec.timeout_ms.unwrap_or(self.ctx.project.timeout_ms),
            insecure: spec.insecure.unwrap_or(false),
            source,
            proxy: self.ctx.proxy.clone(),
        };
        result.curl_cmd = Some(grpc::grpcurl_command(&call, root.as_deref()));

        if opts.dry_run {
            result.body_preview = Some(clip_preview(&call.messages.join("\n")));
            // 配置了 .proto：不连服务也能确认方法存在、消息字段写对了
            if matches!(call.source, grpc::Source::Protos { .. }) && leftover_error(&call.messages).is_none() {
                if let Err(e) = grpc::check_offline(&call.method, &call.messages, &call.source) {
                    result.error = Some(format!("{e:#}"));
                }
            }
            self.redact_result(result);
            return None;
        }

        let reply = match grpc::call(&call) {
            Ok(r) => r,
            Err(e) => {
                result.error = Some(format!("{e:#}"));
                self.redact_result(result);
                return None;
            }
        };
        let mut frames: Vec<crate::engine::Frame> = reply.sent.iter().map(|m| crate::engine::Frame { sent: true, kind: "message".into(), data: m.to_string(), at_ms: 0 }).collect();
        frames.extend(reply.messages.iter().map(|m| crate::engine::Frame { sent: false, kind: "message".into(), data: m.to_string(), at_ms: reply.time_ms }));
        let resp = RespData {
            status: reply.code,
            time_ms: reply.time_ms,
            size: reply.size,
            headers: reply.metadata.clone(),
            body: reply.body().to_string(),
            protocol: Protocol::Grpc,
            frames,
            evidence: None,
        };
        Some(Exchanged { resp, outcomes: vec![] })
    }

    /// WebSocket：握手（被拒时以握手状态作为结果）→ 按步骤收发，receive / waitFor 的断言与捕获即时生效
    #[allow(clippy::too_many_arguments)]
    fn exchange_ws(
        &mut self,
        dc: &discover::DiscoveredCase,
        case: &TestCase,
        row: Option<&crate::config::DataRow>,
        vars: &Vars,
        env_view: &BTreeMap<String, VarVal>,
        result: &mut CaseResult,
        opts: &RunOptions,
    ) -> Option<Exchanged> {
        use crate::engine::ws;
        let spec = case.ws.as_ref().expect("WebSocket 用例");
        result.method = "WS".into();
        let query: Vec<(String, String)> = enabled_kv(&spec.query).map(|(k, v)| (vars.render(k), vars.render(v))).collect();
        let mut url = append_query(&vars.render(&spec.url), &query);
        let mut headers = self.merged_headers(dc, &spec.headers, vars, &["content-type", "content-length"]);
        if !case.pre.is_empty() {
            let req = ReqData { method: "GET".into(), url: url.clone(), headers: headers.iter().cloned().collect(), body: None };
            if let Some(rd) = self.run_pre_script(dc, case, env_view, req, result)? {
                url = rd.url;
                headers = rd.headers.into_iter().collect();
            }
            let vars2 = self.vars_for(&dc.chain);
            url = vars2.render(&url);
            headers = headers.into_iter().map(|(k, v)| (k, vars2.render(&v))).collect();
        }
        result.url = url.clone();
        let http_url = ws::normalize_url(&url).map(|u| u.replacen("ws", "http", 1)).unwrap_or_default();
        if !opts.dry_run {
            let mut texts = vec![url.clone()];
            texts.extend(headers.iter().map(|(_, v)| v.clone()));
            if let Some(e) = leftover_error(&texts) {
                result.error = Some(e);
                return None;
            }
            if self.ctx.project.cookies && !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("cookie")) {
                if let Some(cookie) = self.cookies.header_for(&http_url, crate::history::now_ms()) {
                    headers.push(("Cookie".into(), cookie));
                }
            }
        }
        let conn = ws::WsConnect {
            url,
            headers,
            subprotocols: spec.subprotocols.iter().map(|p| vars.render(p)).collect(),
            timeout_ms: spec.timeout_ms.unwrap_or(self.ctx.project.timeout_ms),
            insecure: spec.insecure.unwrap_or(false),
            proxy: self.ctx.proxy.clone(),
        };
        let sends: Vec<String> = spec.steps.iter().filter_map(|s| s.send.as_ref().map(|v| render_ws_send(v, vars))).collect();
        result.curl_cmd = Some(ws::websocat_command(&conn, &sends));
        if opts.dry_run {
            result.body_preview = Some(clip_preview(&spec.steps.iter().enumerate().map(|(i, s)| format!("#{} {}", i + 1, describe_ws_step(s, vars))).collect::<Vec<_>>().join("\n")));
            self.redact_result(result);
            return None;
        }

        let (mut session, hs) = match ws::connect(&conn) {
            Ok(x) => x,
            Err(e) => {
                result.error = Some(format!("{e:#}"));
                self.redact_result(result);
                return None;
            }
        };
        if self.ctx.project.cookies {
            let set_cookies = hs.headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie")).map(|(_, v)| v.as_str());
            self.cookies.store(&http_url, set_cookies, crate::history::now_ms());
        }
        if hs.status != 101 {
            // 握手被拒：结果就是这次 HTTP 响应（expect.status 写 401 / 403 可断言）
            // 没写 expect.status 却握手失败：对话一步都没进行，不能算通过
            let outcomes = if case.expected_status().is_none() {
                let first = hs.body.lines().next().unwrap_or_default().trim().to_string();
                vec![TestOutcome { expr: "握手 status = 101".into(), pass: false, detail: Some(format!("握手被拒：实际 {}{}", hs.status, if first.is_empty() { String::new() } else { format!("（{first}）") })) }]
            } else {
                vec![]
            };
            let resp = RespData { status: hs.status, time_ms: session.elapsed_ms(), size: hs.body.len() as u64, headers: hs.headers, body: hs.body, protocol: Protocol::Ws, frames: vec![], evidence: None };
            return Some(Exchanged { resp, outcomes });
        }

        let mut outcomes: Vec<TestOutcome> = vec![];
        let default_wait = conn.timeout_ms;
        for (i, step) in spec.steps.iter().enumerate() {
            if Self::cancelled(opts) {
                break;
            }
            let n = i + 1;
            let step_vars = || {
                let mut v = self.vars_for(&dc.chain);
                if let Some(row) = row {
                    v.push_layer(row.clone());
                }
                v
            };
            let wait = Duration::from_millis(step.timeout_ms.unwrap_or(default_wait).max(1));
            let fail = |expr: String, detail: String| TestOutcome { expr, pass: false, detail: Some(detail) };
            let Some(action) = step.action() else {
                outcomes.push(fail(format!("#{n} 步骤"), "每一步只能写一个动作：send / sendBinary / receive / waitFor / wait / close".into()));
                break;
            };
            match action {
                WsAction::Send | WsAction::SendBinary => {
                    let v = step_vars();
                    let text = match (&step.send, &step.send_binary) {
                        (Some(val), _) => render_ws_send(val, &v),
                        (_, Some(b)) => v.render(b),
                        _ => unreachable!(),
                    };
                    let label = format!("#{n} {} {}", action.label(), clip_preview_short(&text));
                    if let Some(e) = leftover_error(std::slice::from_ref(&text)) {
                        outcomes.push(fail(label, e));
                        break;
                    }
                    let sent = if action == WsAction::Send {
                        session.send_text(&text)
                    } else {
                        use base64::Engine;
                        match base64::engine::general_purpose::STANDARD.decode(text.trim()) {
                            Ok(bytes) => session.send_binary(bytes),
                            Err(e) => Err(anyhow::anyhow!("sendBinary 不是合法的 base64：{e}")),
                        }
                    };
                    if let Err(e) = sent {
                        outcomes.push(fail(label, format!("{e:#}")));
                        break;
                    }
                }
                WsAction::Receive | WsAction::WaitFor => {
                    let matcher = if action == WsAction::Receive { step.receive.clone() } else { step.wait_for.clone() }.unwrap_or_default();
                    let v = step_vars();
                    let expect = crate::expect::Expect { body: matcher.clone(), ..Default::default() };
                    let summary = if matcher.is_empty() { "任意消息".to_string() } else { crate::expect::describe_matchers(&matcher, &v) };
                    let label = format!("#{n} {} {summary}", action.label());
                    let started = Instant::now();
                    let mut matched: Option<(usize, Vec<TestOutcome>)> = None;
                    let mut last_fail: Vec<TestOutcome> = vec![];
                    let mut why = String::new();
                    loop {
                        let left = wait.saturating_sub(started.elapsed());
                        match session.recv(left) {
                            Ok(ws::Recv::Message(idx)) => {
                                let data = session.frames[idx].data.clone();
                                let actual = crate::expect::Actual { status: 101, headers: &[], body: &data, protocol: Protocol::Ws };
                                let checks = crate::expect::evaluate(&expect, &actual, &v);
                                if checks.iter().all(|c| c.pass) {
                                    matched = Some((idx, checks));
                                    break;
                                }
                                if action == WsAction::Receive {
                                    last_fail = checks;
                                    break;
                                }
                            }
                            Ok(ws::Recv::Closed) => {
                                let (code, reason) = session.closed_by_server.clone().unwrap_or((1006, String::new()));
                                why = format!("连接已被服务端关闭（{code}{}）", if reason.is_empty() { String::new() } else { format!(" {reason}") });
                                break;
                            }
                            Ok(ws::Recv::Timeout) => {
                                why = format!("{} ms 内没有收到{}的消息", wait.as_millis(), if action == WsAction::WaitFor { "满足条件" } else { "" });
                                break;
                            }
                            Err(e) => {
                                why = format!("{e:#}");
                                break;
                            }
                        }
                    }
                    match matched {
                        Some((idx, checks)) => {
                            outcomes.push(TestOutcome { expr: label, pass: true, detail: None });
                            let _ = checks;
                            if !step.capture.is_empty() {
                                let data = session.frames[idx].data.clone();
                                let actual = crate::expect::Actual { status: 101, headers: &[], body: &data, protocol: Protocol::Ws };
                                let captured = crate::expect::capture(&step.capture, &actual);
                                self.runtime_vars.extend(captured.vars);
                                let failed = !captured.failures.is_empty();
                                outcomes.extend(captured.failures.into_iter().map(|mut f| {
                                    f.expr = format!("#{n} {}", f.expr);
                                    f
                                }));
                                if failed {
                                    break;
                                }
                            }
                        }
                        None if !last_fail.is_empty() => {
                            let detail = last_fail.iter().filter(|c| !c.pass).map(|c| format!("{} → {}", c.expr.replacen("body", "消息", 1), c.detail.clone().unwrap_or_default())).collect::<Vec<_>>().join("；");
                            outcomes.push(fail(label, detail));
                            break;
                        }
                        None => {
                            outcomes.push(fail(label, why));
                            break;
                        }
                    }
                }
                WsAction::Wait => {
                    let _ = session.drain_for(Duration::from_millis(step.wait.unwrap_or(0)));
                }
                WsAction::Close => {
                    let _ = session.close(step.close.as_ref().map(|c| c.code()).unwrap_or(1000));
                }
            }
        }
        if session.is_open() {
            let _ = session.close(1000);
        }
        let received: Vec<serde_json::Value> = session.frames.iter().filter(|f| !f.sent && f.kind != "close").map(ws::frame_value).collect();
        let mut headers = hs.headers;
        if let Some((code, reason)) = &session.closed_by_server {
            headers.push(("ws-close-code".into(), code.to_string()));
            headers.push(("ws-close-reason".into(), reason.clone()));
        }
        let resp = RespData {
            status: hs.status,
            time_ms: session.elapsed_ms(),
            size: session.received_bytes,
            headers,
            body: serde_json::Value::Array(received).to_string(),
            protocol: Protocol::Ws,
            frames: session.frames.clone(),
            evidence: None,
        };
        Some(Exchanged { resp, outcomes })
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
        // 快照：失败的多留一些给 AI 自修复；通过的也留，历史里能回看「当时返回了什么」
        let limit = if result.passed() { 2000 } else { 4000 };
        let binary = is_binary(&resp_data.headers, resp_data.body.as_bytes()) || resp_data.body.contains('\u{FFFD}');
        let body = if binary {
            format!("<二进制响应 {} 字节，未保存到历史；发送该用例可预览 / 另存为>", resp_data.size)
        } else {
            resp_data.body.chars().take(limit).collect::<String>()
        };
        result.response_snapshot = Some(ResponseSnapshot {
            status: resp_data.status,
            headers: resp_data.headers.clone(),
            truncated: !binary && resp_data.body.chars().count() > limit,
            body,
            duration_ms: resp_data.time_ms,
            protocol: resp_data.protocol,
            // 时间线进历史：最多 200 条，每条 500 字符
            frames: resp_data
                .frames
                .iter()
                .take(200)
                .map(|f| {
                    let mut f = f.clone();
                    if f.data.chars().count() > 500 {
                        f.data = format!("{}…", f.data.chars().take(500).collect::<String>());
                    }
                    f
                })
                .collect(),
            evidence: resp_data.evidence.clone(),
        });
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
                "frames": resp_data.frames,
                "evidence": resp_data.evidence,
            });
            let _ = crate::fsx::write(&file, serde_json::to_vec_pretty(&payload).unwrap_or_default());
        }
        self.redact_result(result);
    }

    /// 脚本日志：脱敏后挂到结果上并实时发出 log 事件
    fn collect_logs(&self, result: &mut CaseResult, rel: &str, logs: &[String]) {
        for l in logs {
            let line = self.redact_str(l);
            self.emit(RunEvent::Log { file: rel.to_string(), line: line.clone() });
            result.logs.push(line);
        }
    }

    /// 界面步骤描述里的敏感值（填写的密码等）
    fn redact_evidence(&self, ev: &mut crate::engine::ui::UiEvidence) {
        for s in &mut ev.steps {
            s.title = self.redact_str(&s.title);
            if let Some(d) = &mut s.detail {
                *d = self.redact_str(d);
            }
        }
    }

    /// 界面步骤的实时事件：与最终结果同样先脱敏（填写的密码 / 手机号等 secret 变量值不能出现在界面、终端和事件流里）
    pub(crate) fn emit_ui_step(&self, file: &str, dir: &str, rec: &crate::engine::ui::StepRecord) {
        let mut step = rec.clone();
        step.title = self.redact_str(&step.title);
        if let Some(d) = &mut step.detail {
            *d = self.redact_str(d);
        }
        self.emit(RunEvent::UiStep { file: file.to_string(), dir: dir.to_string(), step });
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
            if let Some(ev) = &mut snap.evidence {
                self.redact_evidence(ev);
            }
        }
        if let Some(fr) = &mut result.full_response {
            fr.body = self.redact_str(&fr.body);
            for (_k, v) in &mut fr.headers {
                *v = self.redact_str(v);
            }
            if let Some(ev) = &mut fr.evidence {
                self.redact_evidence(ev);
            }
        }
        for t in &mut result.tests {
            t.expr = self.redact_str(&t.expr);
            if let Some(d) = &mut t.detail {
                *d = self.redact_str(d);
            }
        }
    }
}

/// WebSocket 要发送的文本：字符串原样（替换变量），其余按 JSON 发送（`"{{id:number}}"` 输出裸数字）
pub fn render_ws_send(v: &serde_json::Value, vars: &Vars) -> String {
    match v {
        serde_json::Value::String(s) => vars.render(s),
        other => vars.render(&other.to_string()),
    }
}

/// 步骤的一行描述（dry-run / 界面）
pub fn describe_ws_step(s: &crate::config::WsStep, vars: &Vars) -> String {
    match s.action() {
        Some(WsAction::Send) => format!("发送 {}", clip_preview_short(&render_ws_send(s.send.as_ref().unwrap(), vars))),
        Some(WsAction::SendBinary) => format!("发送二进制 {} 字节（base64）", s.send_binary.as_ref().map(|b| b.len()).unwrap_or(0)),
        Some(a @ (WsAction::Receive | WsAction::WaitFor)) => {
            let m = if a == WsAction::Receive { &s.receive } else { &s.wait_for };
            let m = m.clone().unwrap_or_default();
            let what = if m.is_empty() { "任意消息".into() } else { crate::expect::describe_matchers(&m, vars) };
            let cap = if s.capture.is_empty() { String::new() } else { format!("，捕获 {}", s.capture.keys().cloned().collect::<Vec<_>>().join("、")) };
            format!("{} {what}{cap}", a.label())
        }
        Some(WsAction::Wait) => format!("静候 {} ms", s.wait.unwrap_or(0)),
        Some(WsAction::Close) => format!("关闭（{}）", s.close.as_ref().map(|c| c.code()).unwrap_or(1000)),
        None => "（每一步只能写一个动作）".into(),
    }
}

fn clip_preview(s: &str) -> String {
    let mut preview: String = s.chars().take(800).collect();
    if s.chars().count() > 800 {
        preview.push('…');
    }
    preview
}

fn clip_preview_short(s: &str) -> String {
    let one = s.replace('\n', " ");
    if one.chars().count() > 60 {
        format!("{}…", one.chars().take(60).collect::<String>())
    } else {
        one
    }
}

/// 真实执行前的残留占位符检查：未定义变量提前清晰报错
fn leftover_error(texts: &[String]) -> Option<String> {
    let mut leftovers: Vec<String> = vec![];
    for t in texts {
        for lo in scan_leftover_placeholders(t) {
            if !leftovers.contains(&lo) {
                leftovers.push(lo);
            }
        }
    }
    if leftovers.is_empty() {
        return None;
    }
    let detail: Vec<String> = leftovers
        .iter()
        .map(|lo| {
            if let Some(name) = lo.strip_prefix('%') {
                format!("环境变量 {name} 未设置（{{{{{lo}}}}} 依赖进程环境变量，请先 export {name}）")
            } else {
                format!("变量 {lo} 未定义（检查 requestbaby.json / envs / collection vars / 前序用例 capture）")
            }
        })
        .collect();
    Some(format!("占位符未替换: {}", detail.join("; ")))
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
    let env = match &root {
        Some(r) => crate::config::load_env(r, env_name)?,
        None => None,
    };
    let env_vars = env.as_ref().map(|e| e.vars.plain()).unwrap_or_default();
    let secret_values = match &root {
        Some(r) => {
            let mut s = load_strict::<Project>(&r.join(discover::PROJECT_FILE))
                .map(|p| p.vars.secret_values())
                .unwrap_or_default();
            s.extend(env.as_ref().map(|e| e.vars.secret_values()).unwrap_or_default());
            s
        }
        None => vec![],
    };
    let proxy = {
        let mut v = crate::vars::Vars::new();
        v.push_str_layer(project.vars.plain());
        v.push_str_layer(env_vars.clone());
        crate::engine::proxy::ProxyPolicy::resolve(project.proxy.as_ref(), |s| v.render(s))?
    };
    Ok(ProjectCtx {
        project,
        env_vars,
        secret_values,
        proxy,
    })
}

#[cfg(test)]
mod session_tests {
    use super::*;

    fn runner() -> (tempfile::TempDir, Runner) {
        let dir = tempfile::tempdir().unwrap();
        crate::scaffold::init_project(dir.path()).unwrap();
        let ctx = load_project_ctx(dir.path(), "dev").unwrap();
        (dir, Runner::new(ctx))
    }

    fn cookie(n: &str) -> crate::cookies::Cookie {
        crate::cookies::Cookie { name: n.into(), value: "v".into(), domain: "h".into(), host_only: true, path: "/".into(), secure: false, expires_at_ms: None }
    }

    /// 切换用户：Cookie 与网页存储各用各的，变量共享；state() 以默认用户为主
    #[test]
    fn switch_user_isolates_login_state_but_shares_vars() {
        let (_d, mut r) = runner();
        r.cookies.insert(cookie("teacher"));
        r.storage.insert("https://h".into(), OriginStorage { local: [("token".to_string(), "t1".to_string())].into(), session: Default::default() });
        r.runtime_vars.insert("taskId".into(), VarVal::Str("7".into()));
        r.switch_user(Some("student"));
        assert!(r.cookies.is_empty() && r.storage.is_empty(), "新用户从空白登录态开始");
        assert!(r.runtime_vars.contains_key("taskId"), "变量共享");
        r.cookies.insert(cookie("student"));
        let st = r.state();
        assert_eq!(st.cookies.iter().map(|c| c.name.clone()).collect::<Vec<_>>(), ["teacher"], "state() 以默认用户为主");
        assert_eq!(st.users["student"].cookies.len(), 1);
        r.switch_user(None);
        assert_eq!(r.cookies.iter().next().unwrap().name, "teacher");
        assert_eq!(r.storage["https://h"].local["token"], "t1");
        r.switch_user(Some("student"));
        assert_eq!(r.cookies.iter().next().unwrap().name, "student", "切回来登录态还在");
        // seed 还原
        let (_d2, mut r2) = runner();
        r2.seed(r.state());
        assert_eq!(r2.cookies.iter().next().unwrap().name, "teacher");
        r2.switch_user(Some("student"));
        assert_eq!(r2.cookies.iter().next().unwrap().name, "student");
    }
}
