//! 多端混合运行的时间线：把一次运行里 API / gRPC / WebSocket / Web / Android 用例的执行过程，
//! 按真实发生的先后排成一条线（每个用例一行；界面用例展开成每一步；脚本输出跟在所属用例后面）。
//! HTML 报告、桌面端、命令行共用这一份计算。

use crate::config::Protocol;
use crate::engine::ui::UiStepStatus;
use crate::report::CaseReport;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EntryKind {
    /// 一个用例（API 用例只有这一行；界面用例是它的起点）
    Case,
    /// 界面用例的一步
    Step,
    /// 脚本 log(...) 输出
    Log,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EntryStatus {
    Passed,
    Failed,
    Skipped,
    /// 仅说明（脚本输出、进行中的用例）
    Info,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineEntry {
    /// 距本次运行第一个用例开始的毫秒数
    pub at_ms: u64,
    pub duration_ms: u64,
    /// 所属用例在结果列表里的下标
    pub case: usize,
    pub protocol: Protocol,
    pub kind: EntryKind,
    pub status: EntryStatus,
    /// 用例名（Case / Log 行）或步骤描述（Step 行）
    pub text: String,
    /// 补充说明：API 的「方法 地址 → 状态」、失败原因、脚本输出的内容
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// 界面用例的第几步
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<usize>,
}

/// 端的名字：API 一类协议统称「API」，其余用各自名字
pub fn side_label(p: Protocol) -> &'static str {
    match p {
        Protocol::Http => "API",
        Protocol::Grpc => "gRPC",
        Protocol::Ws => "WebSocket",
        Protocol::Web => "Web",
        Protocol::Android => "Android",
        Protocol::Desktop => "Mac 桌面",
    }
}

/// 这批结果涉及的不同「端」（按出现顺序）；两种及以上才算多端混合
pub fn sides(results: &[CaseReport]) -> Vec<Protocol> {
    let mut v: Vec<Protocol> = vec![];
    for r in results {
        // 集合 setup / teardown、配置错误这类没有请求的行不算一个「端」
        if r.url == "-" && r.method == "-" && r.protocol == Protocol::Http {
            continue;
        }
        if !v.contains(&r.protocol) {
            v.push(r.protocol);
        }
    }
    v
}

pub fn timeline(results: &[CaseReport]) -> Vec<TimelineEntry> {
    // 用例起止：有结束时刻的用 结束 - 耗时；旧记录（没有时刻）按顺序累加耗时估算
    let mut cursor = 0u64;
    let mut spans: Vec<(u64, u64)> = Vec::with_capacity(results.len());
    for r in results {
        let (start, end) = if r.finished_at_ms > 0 {
            (r.finished_at_ms.saturating_sub(r.duration_ms), r.finished_at_ms)
        } else {
            (cursor, cursor + r.duration_ms)
        };
        cursor = cursor.max(end);
        spans.push((start, end));
    }
    // 原点：第一个用例的开始（旧记录没有时刻，累加值本来就从 0 起）
    let origin = results.iter().zip(&spans).filter(|(r, _)| r.finished_at_ms > 0).map(|(_, s)| s.0).min().unwrap_or(0);
    let rel = |t: u64| t.saturating_sub(origin);

    let mut out: Vec<TimelineEntry> = vec![];
    for (i, r) in results.iter().enumerate() {
        let (start, end) = spans[i];
        let status = if r.skipped {
            EntryStatus::Skipped
        } else if r.passed {
            EntryStatus::Passed
        } else {
            EntryStatus::Failed
        };
        let ev = r.response.as_ref().and_then(|x| x.evidence.as_ref());
        let status_text = if r.status_text.is_empty() { r.status.to_string() } else { r.status_text.clone() };
        let mut detail = match (ev.is_some(), r.url.as_str()) {
            (false, "-") | (false, "") => String::new(),
            (false, url) => format!("{} {url} → {status_text}", r.method),
            (true, url) if url != "-" && !url.is_empty() => url.to_string(),
            _ => String::new(),
        };
        if status == EntryStatus::Failed {
            let why = r.error.clone().or_else(|| r.tests.iter().find(|t| !t.pass).map(|t| t.expr.clone())).unwrap_or_default();
            if !why.is_empty() {
                if !detail.is_empty() {
                    detail.push('\n');
                }
                detail.push_str(&why);
            }
        }
        out.push(TimelineEntry {
            at_ms: rel(start),
            duration_ms: end.saturating_sub(start),
            case: i,
            protocol: r.protocol,
            kind: EntryKind::Case,
            status,
            text: r.name.clone(),
            detail,
            step: None,
        });
        if let Some(ev) = ev {
            for s in &ev.steps {
                let at = (start + s.at_ms).min(end.max(start));
                out.push(TimelineEntry {
                    at_ms: rel(at),
                    duration_ms: s.duration_ms,
                    case: i,
                    protocol: r.protocol,
                    kind: EntryKind::Step,
                    status: match s.status {
                        UiStepStatus::Passed => EntryStatus::Passed,
                        UiStepStatus::Failed => EntryStatus::Failed,
                        UiStepStatus::Skipped => EntryStatus::Skipped,
                    },
                    text: format!("#{} {}", s.index, s.title),
                    detail: if s.status == UiStepStatus::Failed { s.detail.clone().unwrap_or_default() } else { String::new() },
                    step: Some(s.index),
                });
            }
        }
        for line in &r.logs {
            out.push(TimelineEntry {
                at_ms: rel(end),
                duration_ms: 0,
                case: i,
                protocol: r.protocol,
                kind: EntryKind::Log,
                status: EntryStatus::Info,
                text: r.name.clone(),
                detail: line.clone(),
                step: None,
            });
        }
    }
    // 稳定排序：同一时刻保持构造顺序（用例行 → 它的步骤 → 它的脚本输出）
    out.sort_by_key(|e| e.at_ms);
    out
}

/// 纯文本时间线（命令行、贴给 AI / 同事）：`+12.3s  [Web] ✔ #4 点击 按钮「提交」`
pub fn render_text(results: &[CaseReport]) -> String {
    let mut lines = vec![];
    for e in timeline(results) {
        let mark = match e.status {
            EntryStatus::Passed => "✔",
            EntryStatus::Failed => "✘",
            EntryStatus::Skipped => "⇣",
            EntryStatus::Info => "·",
        };
        let indent = if e.kind == EntryKind::Case { "" } else { "  " };
        let body = match e.kind {
            EntryKind::Log => format!("输出 {}", e.detail),
            EntryKind::Case if !e.detail.is_empty() && e.status != EntryStatus::Failed => format!("{}  {}", e.text, e.detail.lines().next().unwrap_or("")),
            _ => e.text.clone(),
        };
        lines.push(format!("+{:>6.1}s  {:<9} {indent}{mark} {body}", e.at_ms as f64 / 1000., format!("[{}]", side_label(e.protocol))));
        if e.status == EntryStatus::Failed && !e.detail.is_empty() {
            for d in e.detail.lines() {
                lines.push(format!("{:>20}{indent}    ↳ {d}", ""));
            }
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::ui::{StepRecord, UiEvidence};
    use crate::runner::ResponseSnapshot;

    fn api(name: &str, finished: u64, dur: u64, passed: bool) -> CaseReport {
        CaseReport {
            file: format!("collections/10-api/{name}.json"),
            name: name.into(),
            tags: vec![],
            method: "POST".into(),
            url: "http://h/x".into(),
            protocol: Protocol::Http,
            status: 200,
            status_text: String::new(),
            duration_ms: dur,
            skipped: false,
            passed,
            error: None,
            curl: None,
            response: None,
            tests: vec![],
            logs: vec![],
            finished_at_ms: finished,
        }
    }

    fn ui(name: &str, p: Protocol, finished: u64, dur: u64, steps: &[(usize, u64, UiStepStatus)]) -> CaseReport {
        let mut c = api(name, finished, dur, steps.iter().all(|s| s.2 != UiStepStatus::Failed));
        c.protocol = p;
        c.method = "-".into();
        c.response = Some(ResponseSnapshot {
            status: 0,
            headers: vec![],
            body: String::new(),
            duration_ms: dur,
            truncated: false,
            protocol: p,
            frames: vec![],
            evidence: Some(UiEvidence {
                dir: "d".into(),
                steps: steps
                    .iter()
                    .map(|(i, at, st)| StepRecord { index: *i, action: "click".into(), title: format!("步骤{i}"), status: *st, at_ms: *at, duration_ms: 10, screenshot: None, detail: None })
                    .collect(),
                environment: vec![],
                artifacts: vec![],
            }),
        });
        c
    }

    #[test]
    fn mixes_api_web_and_android_in_real_order() {
        let r = vec![
            api("下单", 11_000, 500, true),
            ui("安卓支付", Protocol::Android, 18_000, 6_000, &[(1, 1000, UiStepStatus::Passed), (2, 4000, UiStepStatus::Passed)]),
            ui("后台审核", Protocol::Web, 30_000, 8_000, &[(1, 500, UiStepStatus::Passed), (2, 6000, UiStepStatus::Failed)]),
        ];
        let t = timeline(&r);
        // 原点 = 第一个用例的开始（10.5s）
        assert_eq!(t[0].at_ms, 0);
        assert_eq!((t[0].protocol, t[0].kind), (Protocol::Http, EntryKind::Case));
        let order: Vec<(Protocol, EntryKind)> = t.iter().map(|e| (e.protocol, e.kind)).collect();
        assert_eq!(
            order,
            vec![
                (Protocol::Http, EntryKind::Case),
                (Protocol::Android, EntryKind::Case),
                (Protocol::Android, EntryKind::Step),
                (Protocol::Android, EntryKind::Step),
                (Protocol::Web, EntryKind::Case),
                (Protocol::Web, EntryKind::Step),
                (Protocol::Web, EntryKind::Step),
            ]
        );
        assert!(t.windows(2).all(|w| w[0].at_ms <= w[1].at_ms), "必须按时间排序");
        assert_eq!(sides(&r), vec![Protocol::Http, Protocol::Android, Protocol::Web]);
        let failed: Vec<_> = t.iter().filter(|e| e.status == EntryStatus::Failed).map(|e| e.case).collect();
        assert_eq!(failed, vec![2, 2], "失败的用例行与失败的那一步都标红");
    }

    #[test]
    fn script_logs_follow_their_case_and_old_records_fall_back_to_order() {
        let mut a = api("登录", 0, 300, true);
        a.logs = vec!["token=****".into()];
        let b = api("下单", 0, 200, true);
        let t = timeline(&[a, b]);
        assert_eq!(t.iter().map(|e| e.kind).collect::<Vec<_>>(), vec![EntryKind::Case, EntryKind::Log, EntryKind::Case]);
        assert_eq!(t[2].at_ms, 300, "没有时刻的旧记录按耗时顺序累加");
        let text = render_text(&[api("登录", 5000, 300, false)]);
        assert!(text.contains("[API]") && text.contains("✘ 登录"), "{text}");
    }
}
