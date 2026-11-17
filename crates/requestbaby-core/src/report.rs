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
    pub status: u16,
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
            status: r.status,
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
    /// 工作流运行时为工作流 key
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
}

/// 工作流步骤状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StepStatus {
    Passed,
    Failed,
    /// 前序步骤失败或运行被取消，未执行
    NotRun,
}

/// 工作流单步汇总（协议形态）
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
    /// 工作流运行的分步汇总（普通运行为空）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<StepReport>,
    pub results: Vec<CaseReport>,
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
            results,
        }
    }

    pub fn duration_ms(&self) -> u64 {
        self.results.iter().map(|r| r.duration_ms).sum()
    }
}

/// 单文件 HTML（内嵌协议数据 + 极简渲染）
pub fn render_html(report: &RunReport) -> String {
    let data = render_json(report);
    format!(
        r#"<!DOCTYPE html><html lang="zh"><head><meta charset="utf-8"><title>requestbaby report</title>
<style>
body{{font-family:-apple-system,'PingFang SC',monospace;margin:24px;background:#0f172a;color:#e2e8f0}}
table{{border-collapse:collapse;width:100%}}
td,th{{padding:6px 10px;border-bottom:1px solid #1e293b;text-align:left;font-size:13px}}
.pass{{color:#4ade80}}.fail{{color:#f87171}}.skip{{color:#fbbf24}}.dim{{color:#64748b}}
details{{margin:4px 0}}pre{{background:#1e293b;padding:8px;border-radius:6px;overflow:auto;font-size:12px}}
h1{{font-size:18px}}
</style></head><body>
<h1>requestbaby 报告</h1>
<table id="t"></table>
<script>
const R = {data};
const t = document.getElementById('t');
t.innerHTML = '<tr><th>结果</th><th>用例</th><th>方法</th><th>URL</th><th>状态</th><th>耗时</th></tr>' +
R.results.map(r => {{
const cls = r.skipped ? 'skip' : (r.passed ? 'pass' : 'fail');
const mark = r.skipped ? '⇣' : (r.passed ? '✔' : '✘');
const detail = (r.error ? `<div class="fail">${{esc(r.error)}}</div>` : '') +
(r.tests||[]).filter(x=>!x.pass).map(x=>`<div>✘ ${{esc(x.expr)}}<pre>${{esc(x.detail||'')}}</pre></div>`).join('');
return `<tr><td class="${{cls}}">${{mark}}</td><td>${{esc(r.name)}}${{detail?`<details><summary class="dim">详情</summary>${{detail}}</details>`:''}}</td><td class="dim">${{r.method}}</td><td>${{esc(r.url)}}</td><td class="${{r.status>=400?'fail':'pass'}}">${{r.status||'-'}}</td><td class="dim">${{r.duration_ms}}ms</td></tr>`;
}}).join('');
function esc(s){{return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;')}}
</script></body></html>"#
    )
}

/// 协议 JSON（`--report json`、Desktop 导出、历史文件同一形态）
pub fn render_json(report: &RunReport) -> String {
    serde_json::to_string_pretty(report).unwrap_or_default()
}
