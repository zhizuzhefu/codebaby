use crate::runner::{CaseResult, ResponseSnapshot};
use serde::{Deserialize, Serialize};

/// 报告协议版本：`--report json`、运行历史文件、`runFinished` 事件三处一致
pub const REPORT_SCHEMA: &str = "requestbaby/run@1";

/// 单条断言（协议形态）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestReport {
    pub expr: String,
    pub pass: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// 单用例结果（协议形态）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseReport {
    pub file: String,
    pub name: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub method: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "crate::config::Protocol::is_http")]
    pub protocol: crate::config::Protocol,
    /// HTTP 状态码 / WebSocket 握手状态 / gRPC 状态码（0 = OK，名字见 status_text）
    pub status: u16,
    /// gRPC 状态名（OK / NOT_FOUND…）；HTTP 与 WebSocket 不输出
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub status_text: String,
    pub duration_ms: u64,
    pub skipped: bool,
    pub passed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curl: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<ResponseSnapshot>,
    #[serde(default)]
    pub tests: Vec<TestReport>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub logs: Vec<String>,
    /// 用例结束的时刻（Unix 毫秒）；缺省（旧记录）时时间线按顺序累加耗时估算
    #[serde(default, skip_serializing_if = "is_zero")]
    pub finished_at_ms: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl CaseReport {
    /// include_curl：是否携带编译出的 curl 命令（`-v` / 历史记录）
    pub fn from_result(r: &CaseResult, include_curl: bool) -> Self {
        Self {
            file: r.rel.clone(),
            name: r.name.clone(),
            tags: r.tags.clone(),
            method: r.method.clone(),
            url: r.url.clone(),
            protocol: r.protocol,
            status: r.status,
            status_text: if matches!(r.protocol, crate::config::Protocol::Grpc | crate::config::Protocol::Web | crate::config::Protocol::Android | crate::config::Protocol::Desktop) { r.status_text() } else { String::new() },
            duration_ms: r.duration_ms,
            skipped: r.skipped,
            passed: r.passed(),
            error: r.error.clone(),
            curl: if include_curl { r.curl_cmd.clone() } else { None },
            response: r.response_snapshot.clone(),
            tests: r
                .tests
                .iter()
                .map(|t| TestReport { expr: t.expr.clone(), pass: t.pass, detail: t.detail.clone() })
                .collect(),
            logs: r.logs.clone(),
            finished_at_ms: r.finished_at_ms,
        }
    }

    /// 失败摘要（纯文本，贴给 AI / 同事）；集合 setup / teardown 没有请求，不输出「方法 URL → 状态」行
    pub fn failure_text(&self) -> String {
        let mut text = if self.url.is_empty() || self.url == "-" {
            format!("{} 失败（{}）", self.name, self.file)
        } else {
            let status = if self.status_text.is_empty() { self.status.to_string() } else { self.status_text.clone() };
            format!("用例 {} 失败（{}）\n{} {} → {status}", self.name, self.file, self.method, self.url)
        };
        if let Some(e) = &self.error {
            text.push_str(&format!("\n错误: {e}"));
        }
        for t in self.tests.iter().filter(|t| !t.pass) {
            text.push_str(&format!("\n✘ {}", t.expr));
            if let Some(d) = &t.detail {
                text.push_str(&format!("\n    ↳ {d}"));
            }
        }
        // 界面用例：失败那一步的截图与现场（AI 可以直接打开图片看）
        if let Some(ev) = self.response.as_ref().and_then(|r| r.evidence.as_ref()) {
            if let Some(shot) = ev.failed_step().and_then(|s| s.screenshot.as_ref()) {
                text.push_str(&format!("\n失败截图: {}/{shot}", ev.dir));
            }
            if !ev.artifacts.is_empty() {
                text.push_str(&format!("\n失败现场: {}", ev.artifacts.iter().map(|a| format!("{}/{}", ev.dir, a.file)).collect::<Vec<_>>().join("  ")));
            }
        }
        text
    }
}

/// 运行元信息（谁、何时、在哪个环境、跑了什么）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunMeta {
    pub id: String,
    /// cli / desktop / ci
    pub source: String,
    /// 等价 CLI 命令（GUI 发起的运行同样记录，便于 AI/CI 复现）
    pub command: String,
    pub env: String,
    pub started_at_ms: u64,
    pub finished_at_ms: u64,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub cancelled: bool,
    /// 流程运行时为流程 key
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
    /// `requestbaby verify` 的整体验证（steps 为各阶段）
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub verify: bool,
    /// 运行时的代码版本
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitInfo>,
}

/// 代码版本：提交 / 分支 / 工作区是否有未提交的改动
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitInfo {
    pub commit: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub branch: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dirty: bool,
}

impl GitInfo {
    /// `main@3f2a1c9`（有未提交改动加 `+改动`）
    pub fn short(&self) -> String {
        let c: String = self.commit.chars().take(7).collect();
        let mut s = if self.branch.is_empty() { c } else { format!("{}@{c}", self.branch) };
        if self.dirty {
            s.push_str("+改动");
        }
        s
    }
}

/// 流程步骤状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StepStatus {
    Passed,
    Failed,
    /// 前序步骤失败或运行被取消，未执行
    NotRun,
}

/// 流程单步汇总（协议形态）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepReport {
    pub label: String,
    pub run: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name_filter: String,
    pub status: StepStatus,
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
}

/// 运行报告（协议形态）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunReport {
    #[serde(rename = "$schema", default)]
    pub schema: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<RunMeta>,
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    /// 流程运行的分步汇总 / verify 的阶段汇总（普通运行为空）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<StepReport>,
    /// verify 的校验问题（error 即阻断执行）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<IssueReport>,
    /// verify 无法给出结论的原因（校验错误 / 服务启动失败 / 阶段配置错误）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<String>,
    pub results: Vec<CaseReport>,
    /// 与历史对比（每个用例本次之前最近一次的结论；首次运行 / dry-run 为空）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compare: Option<Comparison>,
}

/// 用例标识：文件 + 名称（数据驱动每行名称不同）
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CaseRef {
    pub file: String,
    pub name: String,
}

/// 本次运行相对历史的变化
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    /// 新失败：之前通过（或从未执行过）、本次失败——优先看这些
    pub new_failures: Vec<CaseRef>,
    /// 已修复：之前失败、本次通过
    pub fixed: Vec<CaseRef>,
    /// 仍失败：之前失败、本次仍失败
    pub still_failing: Vec<CaseRef>,
    /// 不稳定：同一提交上时过时不过（结论不可信，需要人看）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flaky: Vec<CaseRef>,
}

impl Comparison {
    pub fn is_empty(&self) -> bool {
        self.new_failures.is_empty() && self.fixed.is_empty() && self.still_failing.is_empty() && self.flaky.is_empty()
    }

    pub fn is_flaky(&self, file: &str, name: &str) -> bool {
        self.flaky.iter().any(|c| c.file == file && c.name == name)
    }

    /// 某用例在对比中的标记
    pub fn mark(&self, file: &str, name: &str) -> Option<CompareMark> {
        let hit = |v: &[CaseRef]| v.iter().any(|c| c.file == file && c.name == name);
        if hit(&self.new_failures) {
            Some(CompareMark::NewFailure)
        } else if hit(&self.fixed) {
            Some(CompareMark::Fixed)
        } else if hit(&self.still_failing) {
            Some(CompareMark::StillFailing)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareMark {
    NewFailure,
    Fixed,
    StillFailing,
}

impl CompareMark {
    pub fn label(self) -> &'static str {
        match self {
            CompareMark::NewFailure => "新失败",
            CompareMark::Fixed => "已修复",
            CompareMark::StillFailing => "仍失败",
        }
    }
}

/// 校验问题（协议形态，与 `validate --report json` 的 issues 同构）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueReport {
    pub file: String,
    pub level: String,
    pub rule: String,
    pub message: String,
}

impl From<&crate::validate::ValidationIssue> for IssueReport {
    fn from(i: &crate::validate::ValidationIssue) -> Self {
        Self { file: i.file.clone(), level: i.level.into(), rule: i.rule.into(), message: i.message.clone() }
    }
}

impl RunReport {
    pub fn new(results: &[CaseResult], include_curl: bool, meta: Option<RunMeta>) -> Self {
        Self::from_cases(results.iter().map(|r| CaseReport::from_result(r, include_curl)).collect(), meta)
    }

    pub fn from_cases(results: Vec<CaseReport>, meta: Option<RunMeta>) -> Self {
        Self {
            schema: REPORT_SCHEMA.into(),
            meta,
            total: results.len(),
            passed: results.iter().filter(|r| r.passed).count(),
            failed: results.iter().filter(|r| !r.passed && !r.skipped).count(),
            skipped: results.iter().filter(|r| r.skipped).count(),
            steps: vec![],
            issues: vec![],
            blocked: None,
            results,
            compare: None,
        }
    }

    /// 整次运行的失败摘要：无法给出结论的原因 + 校验错误 + 每个失败用例；没有问题时为空
    pub fn failure_text(&self) -> String {
        let mut parts: Vec<String> = vec![];
        if let Some(m) = &self.meta {
            parts.push(format!("{}（环境 {}）", m.command, m.env));
        }
        if let Some(b) = &self.blocked {
            parts.push(format!("无法完成: {b}"));
        }
        let issues: Vec<String> =
            self.issues.iter().filter(|i| i.level == "error").map(|i| format!("✘ {} [{}] {}", i.file, i.rule, i.message)).collect();
        if !issues.is_empty() {
            parts.push(format!("校验错误\n{}", issues.join("\n")));
        }
        let failed: Vec<String> = self.results.iter().filter(|r| !r.passed && !r.skipped).map(CaseReport::failure_text).collect();
        if self.blocked.is_none() && issues.is_empty() && failed.is_empty() {
            return String::new();
        }
        if !failed.is_empty() {
            parts.push(format!("{}/{} 个用例失败", failed.len(), self.total));
        }
        parts.extend(failed);
        parts.join("\n\n")
    }

    pub fn duration_ms(&self) -> u64 {
        self.results.iter().map(|r| r.duration_ms).sum()
    }
}

/// 单文件 HTML 报告（模板见 report.html）：左侧目录 + 每个用例完整展开；界面用例的每一步与截图都在里面，
/// 截图以 data URI 内嵌（root = 项目根），单独发给别人也能看，并带可翻页、可原始大小查看的图片查看器
pub fn render_html(report: &RunReport, root: Option<&std::path::Path>) -> String {
    // 数据 = 协议 JSON + 时间线（多端混合时按真实先后排好）+ 失败现场文件的内容（logcat / 控制台 / 网络请求…，单独发给别人也能看）
    let mut data = serde_json::to_value(report).unwrap_or_default();
    if let Some(obj) = data.as_object_mut() {
        obj.insert("timeline".into(), serde_json::to_value(crate::timeline::timeline(&report.results)).unwrap_or_default());
        obj.insert("sides".into(), serde_json::to_value(crate::timeline::sides(&report.results)).unwrap_or_default());
        obj.insert("artifactText".into(), serde_json::Value::Object(artifact_texts(report, root)));
    }
    let data = serde_json::to_string_pretty(&data).unwrap_or_default().replace("</", "<\\/");
    let mut images = serde_json::Map::new();
    for ev in report.results.iter().filter_map(|r| r.response.as_ref()?.evidence.as_ref()) {
        for shot in ev.steps.iter().filter_map(|s| s.screenshot.as_ref()) {
            if let Ok(bytes) = std::fs::read(ev.file(root, shot)) {
                use base64::Engine;
                let uri = format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes));
                images.insert(format!("{}/{shot}", ev.dir), serde_json::Value::String(uri));
            }
        }
    }
    let images = serde_json::Value::Object(images).to_string();
    let project = root
        .and_then(|r| r.file_name())
        .map(|n| n.to_string_lossy().replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;"))
        .unwrap_or_default();
    // 先替换模板里的项目名，再依次嵌入数据（用户数据里即使出现占位符字样也不会被再次替换）
    let (head, rest) = HTML_TEMPLATE.replacen("__PROJECT__", &project, 1).split_once("__DATA__").map(|(a, b)| (a.to_string(), b.to_string())).expect("report.html 缺 __DATA__");
    let (mid, tail) = rest.split_once("__IMAGES__").expect("report.html 缺 __IMAGES__");
    format!("{head}{data}{mid}{images}{tail}")
}

const HTML_TEMPLATE: &str = include_str!("report.html");

/// 每个文件嵌入的字符上限（页面 HTML 取开头，日志类取结尾——最近发生的最有用）与整份报告的总上限
const ARTIFACT_CHARS: usize = 200_000;
const ARTIFACT_TOTAL_CHARS: usize = 3_000_000;

/// 失败现场里的文本文件（页面结构、控制台、网络请求、logcat、崩溃日志…）：键为「证据目录/文件名」
fn artifact_texts(report: &RunReport, root: Option<&std::path::Path>) -> serde_json::Map<String, serde_json::Value> {
    let mut out = serde_json::Map::new();
    let mut total = 0usize;
    for ev in report.results.iter().filter_map(|r| r.response.as_ref()?.evidence.as_ref()) {
        for a in &ev.artifacts {
            let Ok(bytes) = std::fs::read(ev.file(root, &a.file)) else { continue };
            let text = String::from_utf8_lossy(&bytes);
            let n = text.chars().count();
            let keep_head = matches!(a.file.rsplit('.').next(), Some("html" | "xml"));
            let shown: String = match (n > ARTIFACT_CHARS, keep_head) {
                (false, _) => text.into_owned(),
                (true, true) => format!("{}\n…（内容太长，只保留开头 {ARTIFACT_CHARS} 个字符）", text.chars().take(ARTIFACT_CHARS).collect::<String>()),
                (true, false) => format!("…（内容太长，只保留结尾 {ARTIFACT_CHARS} 个字符）\n{}", text.chars().skip(n - ARTIFACT_CHARS).collect::<String>()),
            };
            if total + shown.chars().count() > ARTIFACT_TOTAL_CHARS {
                continue;
            }
            total += shown.chars().count();
            out.insert(format!("{}/{}", ev.dir, a.file), serde_json::Value::String(shown));
        }
    }
    out
}

/// 协议 JSON（`--report json`、Desktop 导出、历史文件同一形态）
pub fn render_json(report: &RunReport) -> String {
    serde_json::to_string_pretty(report).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(name: &str, url: &str, passed: bool) -> CaseReport {
        CaseReport {
            file: format!("collections/{name}.json"),
            name: name.into(),
            tags: vec![],
            method: "GET".into(),
            url: url.into(),
            protocol: Default::default(),
            status: if url.is_empty() { 0 } else { 500 },
            status_text: String::new(),
            duration_ms: 1,
            skipped: false,
            passed,
            error: None,
            curl: None,
            response: None,
            tests: vec![],
            logs: vec![],
            finished_at_ms: 0,
        }
    }

    #[test]
    fn failure_text_covers_setup_assertions_and_blocked() {
        let mut setup = case("集合 setup：01-readonly", "", false);
        setup.error = Some("setup 脚本失败: Error: 只能使用 local".into());
        let mut api = case("查询", "http://h/x", false);
        api.tests = vec![
            TestReport { expr: "res.status === 200".into(), pass: false, detail: Some("实际 500".into()) },
            TestReport { expr: "true".into(), pass: true, detail: None },
        ];
        let mut report = RunReport::from_cases(vec![setup, api, case("ok", "http://h/ok", true)], None);
        let text = report.failure_text();
        assert!(text.starts_with("2/3 个用例失败"), "{text}");
        assert!(text.contains("集合 setup：01-readonly 失败（collections/集合 setup：01-readonly.json）\n错误: setup 脚本失败"), "{text}");
        assert!(!text.contains("- - → 0") && !text.contains(" → 0"), "{text}");
        assert!(text.contains("GET http://h/x → 500\n✘ res.status === 200\n    ↳ 实际 500"), "{text}");
        assert!(!text.contains("✘ true"));

        report.blocked = Some("校验未通过".into());
        assert!(report.failure_text().contains("无法完成: 校验未通过"));
        let clean = RunReport::from_cases(vec![case("ok", "http://h/ok", true)], None);
        assert!(clean.failure_text().is_empty());
    }

    #[test]
    fn html_embeds_data_and_has_no_leftover_placeholders() {
        let tricky = case("含 </script> 与 __IMAGES__ 字样", "http://h/x", false);
        let report = RunReport::from_cases(vec![tricky, case("ok", "http://h/ok", true)], None);
        let html = render_html(&report, Some(std::path::Path::new("/p/我的<项目>")));
        assert!(html.contains("const R = {") && html.contains("openLB"), "查看器与数据都在");
        assert!(html.contains("data-project=\"我的&lt;项目&gt;\""));
        // 用户数据里的 </script> 不会提前结束脚本；占位符字样只是数据，不被二次替换
        assert!(!html.contains("含 </script>") && html.contains("含 <\\/script> 与 __IMAGES__ 字样"));
        assert!(!html.contains("__DATA__") && !html.contains("__PROJECT__"));
    }
}
