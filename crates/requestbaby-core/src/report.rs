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
            status_text: if matches!(r.protocol, crate::config::Protocol::Grpc | crate::config::Protocol::Web | crate::config::Protocol::Android) { r.status_text() } else { String::new() },
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

/// 单文件 HTML（内嵌协议数据 + 极简渲染）。root = 项目根：界面用例的截图以 data URI 内嵌，报告单独发给别人也能看
pub fn render_html(report: &RunReport, root: Option<&std::path::Path>) -> String {
    let data = render_json(report).replace("</", "<\\/");
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
    format!(
        r#"<!DOCTYPE html><html lang="zh"><head><meta charset="utf-8"><title>requestbaby 验收报告</title>
<style>
body{{font-family:-apple-system,'PingFang SC',sans-serif;margin:24px;background:#0f172a;color:#e2e8f0}}
table{{border-collapse:collapse;width:100%}}
td,th{{padding:6px 10px;border-bottom:1px solid #1e293b;text-align:left;font-size:13px;vertical-align:top}}th{{white-space:nowrap}}
.pass{{color:#4ade80}}.fail{{color:#f87171}}.skip{{color:#fbbf24}}.dim{{color:#64748b}}
details{{margin:4px 0}}pre{{background:#1e293b;padding:8px;border-radius:6px;overflow:auto;font-size:12px;white-space:pre-wrap}}
h1{{font-size:18px;margin-bottom:4px}}.meta{{font-size:12px;margin-bottom:16px}}
.steps{{display:flex;flex-direction:column;gap:10px;margin:8px 0}}
.step{{display:flex;gap:12px;background:#111827;border:1px solid #1e293b;border-radius:8px;padding:8px}}
.step.failed{{border-color:#f87171}}
.step img{{width:320px;max-width:40vw;border-radius:4px;cursor:zoom-in;border:1px solid #334155}}
.step .t{{font-size:13px}}.step .d{{font-size:12px;margin-top:4px}}
.env{{font-size:12px}}.env span{{margin-right:14px}}
#zoom{{position:fixed;inset:0;background:rgba(0,0,0,.85);display:none;align-items:center;justify-content:center;cursor:zoom-out}}
#zoom img{{max-width:95vw;max-height:95vh}}
</style></head><body>
<h1>requestbaby 验收报告</h1>
<div class="meta dim" id="meta"></div>
<table id="t"></table>
<div id="zoom" onclick="this.style.display='none'"><img></div>
<script>
const R = {data};
const IMG = {images};
const t = document.getElementById('t');
const m = R.meta;
if (m) {{
  const g = m.git ? ` · 代码 ${{esc((m.git.branch ? m.git.branch + '@' : '') + m.git.commit.slice(0, 7))}}${{m.git.dirty ? '（有未提交改动）' : ''}}` : '';
  document.getElementById('meta').innerHTML = `${{esc(m.command)}} · 环境 ${{esc(m.env)}} · ${{new Date(m.startedAtMs).toLocaleString()}}${{g}} · ${{R.passed}}/${{R.total}} 通过`;
}}
const flaky = new Set(((R.compare||{{}}).flaky||[]).map(c => c.file + '\u0000' + c.name));
const bad = r => r.protocol==='grpc' ? r.status_text!=='OK' : r.protocol==='ws' ? r.status!==101 : r.protocol==='android' ? false : (r.status>=400 || (!r.status && r.protocol!=='web'));
t.innerHTML = '<tr><th>结果</th><th>用例</th><th>方法</th><th>地址</th><th>状态</th><th>耗时</th></tr>' +
R.results.map(r => {{
  const cls = r.skipped ? 'skip' : (r.passed ? 'pass' : 'fail');
  const mark = r.skipped ? '⇣' : (r.passed ? '✔' : '✘');
  const ev = r.response && r.response.evidence;
  let detail = (r.error ? `<div class="fail">${{esc(r.error)}}</div>` : '') +
    (r.tests||[]).filter(x=>!x.pass).map(x=>`<div>✘ ${{esc(x.expr)}}<pre>${{esc(x.detail||'')}}</pre></div>`).join('');
  if (ev) detail += steps(ev);
  const open = ev && !r.passed ? ' open' : '';
  const tag = flaky.has(r.file + '\u0000' + r.name) ? ' <span class="skip">不稳定</span>' : '';
  return `<tr><td class="${{cls}}">${{mark}}</td><td>${{esc(r.name)}}${{tag}}${{detail?`<details${{open}}><summary class="dim">${{ev ? '步骤与截图（' + ev.steps.length + '）' : '详情'}}</summary>${{detail}}</details>`:''}}</td><td class="dim">${{r.method}}</td><td>${{esc(r.url)}}</td><td class="${{bad(r)?'fail':'pass'}}">${{r.status_text||r.status||'-'}}</td><td class="dim">${{r.duration_ms}}ms</td></tr>`;
}}).join('');
function steps(ev) {{
  const env = (ev.environment||[]).map(([k,v]) => `<span><span class="dim">${{esc(k)}}</span> ${{esc(v)}}</span>`).join('');
  const arts = (ev.artifacts||[]).map(a => `<span>${{esc(a.label)}}：${{esc(ev.dir + '/' + a.file)}}</span>`).join('<br>');
  return `<div class="env">${{env}}</div><div class="steps">` + ev.steps.map(s => {{
    const src = s.screenshot && IMG[ev.dir + '/' + s.screenshot];
    const img = src ? `<img src="${{src}}" onclick="zoom(this.src)">` : (s.screenshot ? `<span class="dim">${{esc(ev.dir + '/' + s.screenshot)}}</span>` : '');
    const st = s.status === 'failed' ? '<span class="fail">✘</span>' : s.status === 'skipped' ? '<span class="skip">⇣</span>' : '<span class="pass">✔</span>';
    return `<div class="step ${{s.status}}">${{img}}<div><div class="t">${{st}} <span class="dim">#${{s.index}}</span> ${{esc(s.title)}}</div><div class="d dim">${{(s.atMs/1000).toFixed(1)}}s · ${{s.durationMs}}ms</div>${{s.detail ? `<div class="d ${{s.status==='failed'?'fail':'dim'}}">${{esc(s.detail)}}</div>` : ''}}</div></div>`;
  }}).join('') + `</div>` + (arts ? `<div class="env"><div class="dim">失败现场</div>${{arts}}</div>` : '') + `<div class="env dim">证据目录 ${{esc(ev.dir)}}</div>`;
}}
function zoom(src) {{ const z = document.getElementById('zoom'); z.querySelector('img').src = src; z.style.display = 'flex'; }}
function esc(s){{return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;')}}
</script></body></html>"#
    )
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
}
