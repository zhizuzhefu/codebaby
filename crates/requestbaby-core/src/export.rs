//! 导出结果：把验收结果交给别人（CLI `requestbaby export` 与 Desktop「导出结果」同一实现）
//!
//! 默认导出「全部用例的最新结果」（逐个集合分别跑过也是一份文件）；可以只导出某个集合，或某一次运行。
//! HTML 是单个文件：每个用例的全部步骤与截图都在里面（带图片查看器），发给别人直接用浏览器打开。
use crate::history;
use crate::report::{RunReport, render_html, render_json};
use anyhow::{Result, bail};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Html,
    Json,
    /// 纯文本：结论、按时间先后排的多端时间线、失败详情（贴进聊天 / 工单 / 给 AI）
    Text,
}

impl Format {
    pub fn parse(s: &str) -> Result<Format> {
        match s.to_ascii_lowercase().as_str() {
            "html" => Ok(Format::Html),
            "json" => Ok(Format::Json),
            "text" | "txt" => Ok(Format::Text),
            other => bail!("不支持的导出格式 {other}（可选 html / json / text）"),
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            Format::Html => "html",
            Format::Json => "json",
            Format::Text => "txt",
        }
    }

    /// 命令行 `-f` 的取值
    pub fn name(self) -> &'static str {
        match self {
            Format::Html => "html",
            Format::Json => "json",
            Format::Text => "text",
        }
    }
}

/// 导出哪些结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// 环境下每个用例最近一次的结果；collections 非空 = 只要这些集合目录下的（如 `collections/03-布置作业`）
    Latest { env: String, collections: Vec<String> },
    /// 某一次运行（运行 ID 见 `requestbaby history`）
    Run(String),
}

/// 取出要导出的报告
pub fn collect(root: &Path, scope: &Scope) -> Result<RunReport> {
    match scope {
        Scope::Latest { env, collections: wanted } => {
            let dirs: Vec<String> = wanted.iter().map(|c| normalize_collection(root, c)).collect();
            if let Some(r) = history::latest_report(root, env, &dirs) {
                // 指定的集合里有没结果的：不静默漏掉，直接说
                let have = self::collections(&r);
                let missing: Vec<&String> = dirs.iter().filter(|d| !have.iter().any(|(h, _)| h == *d || h.starts_with(&format!("{d}/")))).collect();
                if missing.is_empty() {
                    return Ok(r);
                }
                bail!("环境 {env} 下这些集合没有运行结果：{}（先运行它们，或从 -c 里去掉）", missing.iter().map(|m| m.as_str()).collect::<Vec<_>>().join("、"));
            }
            match history::latest_report(root, env, &[]) {
                Some(all) if !dirs.is_empty() => {
                    let have: Vec<String> = self::collections(&all).into_iter().map(|(d, n)| format!("{d}（{n} 个用例）")).collect();
                    bail!("环境 {env} 下集合 {} 没有运行结果；有结果的集合：{}", dirs.join("、"), have.join("、"))
                }
                _ => bail!("环境 {env} 还没有可导出的运行结果：先运行（requestbaby run -e {env}），或用 -e 指定其他环境"),
            }
        }
        Scope::Run(id) => history::find(root, id),
    }
}

/// 集合目录写法归一：接受 `collections/03-布置作业`、`03-布置作业`、带尾部斜杠、项目内绝对路径
pub fn normalize_collection(root: &Path, input: &str) -> String {
    let p = Path::new(input.trim());
    let rel = p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/");
    let rel = rel.trim_matches('/').trim_start_matches("./").to_string();
    if !rel.starts_with("collections") && root.join("collections").join(&rel).is_dir() {
        format!("collections/{rel}")
    } else {
        rel
    }
}

/// 报告里有结果的集合：(目录, 用例数)，按目录名排序；集合 = collections/ 下的第一级目录
pub fn collections(report: &RunReport) -> Vec<(String, usize)> {
    let mut count: std::collections::BTreeMap<String, usize> = Default::default();
    for r in &report.results {
        if let Some(dir) = top_dir(&r.file) {
            *count.entry(dir).or_default() += 1;
        }
    }
    count.into_iter().collect()
}

fn top_dir(file: &str) -> Option<String> {
    let mut it = file.split('/');
    let (a, b) = (it.next()?, it.next()?);
    // 目录下还有一层才是「集合里的用例」；collections/x.json 这样的根级用例不属于任何集合
    it.next()?;
    Some(format!("{a}/{b}"))
}

pub fn render(report: &RunReport, root: &Path, format: Format) -> String {
    match format {
        Format::Html => render_html(report, Some(root)),
        Format::Json => render_json(report),
        Format::Text => render_text(report),
    }
}

/// 纯文本报告：一行结论、时间线（多端按真实先后）、失败摘要
fn render_text(report: &RunReport) -> String {
    let mut out = vec![format!("{} 个用例：{} 通过 · {} 失败{}", report.total, report.passed, report.failed, if report.skipped > 0 { format!(" · {} 跳过", report.skipped) } else { String::new() })];
    if let Some(m) = &report.meta {
        out.push(format!("环境 {} · {}", m.env, m.command));
    }
    out.push(String::new());
    out.push("时间线（按真实发生的先后）".into());
    out.push(crate::timeline::render_text(&report.results));
    let failures = report.failure_text();
    if !failures.is_empty() {
        out.push(String::new());
        out.push("失败详情".into());
        out.push(failures);
    }
    out.push(String::new());
    out.join("\n")
}

/// 汇总报告是按哪些集合取的（从记录的等价命令里读 `-c`）；空 = 全部
pub fn collections_of_command(report: &RunReport) -> Vec<String> {
    let Some(m) = report.meta.as_ref().filter(|m| m.source == "history") else { return vec![] };
    m.command.split(" -c ").skip(1).map(|c| c.split(" -").next().unwrap_or(c).trim().to_string()).collect()
}

/// 默认文件名：`验收报告-<项目>[-<集合>]-<环境>.html`（多个集合写「N个集合」；单次运行用运行 ID）
pub fn file_name(root: &Path, report: &RunReport, format: Format) -> String {
    let project = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "project".into());
    let m = report.meta.as_ref();
    let tail = match m {
        Some(m) if m.source == "history" => {
            let picked = collections_of_command(report);
            match picked.as_slice() {
                [] => m.env.clone(),
                [one] => format!("{}-{}", one.rsplit('/').next().unwrap_or(one.as_str()), m.env),
                many => format!("{}个集合-{}", many.len(), m.env),
            }
        }
        Some(m) => m.id.clone(),
        None => "report".into(),
    };
    format!("验收报告-{project}-{tail}.{}", format.ext())
}

/// 导出前给人看的概况：包含什么、哪里有缺口
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overview {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    /// 项目里有、但这份报告没有结果的用例文件（没跑过）
    pub never_run: Vec<String>,
    /// 报告引用的截图中已被历史清理删掉的张数（HTML 里只剩文字）
    pub missing_screenshots: usize,
    /// 汇总报告里最早 / 最近一条结果的产生时间（毫秒）；单次运行时两者就是这次运行的起止
    pub oldest_ms: u64,
    pub newest_ms: u64,
}

impl Overview {
    /// collections：报告只含某些集合时传它们的目录，「没跑过的用例」只统计这些集合
    pub fn of(report: &RunReport, root: &Path, collections: &[String]) -> Overview {
        let known: std::collections::HashSet<&str> = report.results.iter().map(|r| r.file.as_str()).collect();
        let scope: Vec<String> = collections.iter().map(|c| format!("{}/", c.trim_end_matches('/'))).collect();
        let all = crate::runner::discover::discover(&[root.join("collections")]).unwrap_or_default();
        let never_run = all
            .into_iter()
            .map(|c| c.rel)
            .filter(|rel| (scope.is_empty() || scope.iter().any(|p| rel.starts_with(p.as_str()))) && !known.contains(rel.as_str()))
            .collect();
        let missing_screenshots = report
            .results
            .iter()
            .filter_map(|r| r.response.as_ref()?.evidence.as_ref())
            .flat_map(|ev| ev.steps.iter().filter_map(|s| s.screenshot.as_ref()).map(move |shot| ev.file(Some(root), shot)))
            .filter(|p| !p.is_file())
            .count();
        let (oldest_ms, newest_ms) = report.meta.as_ref().map(|m| (m.started_at_ms, m.finished_at_ms)).unwrap_or_default();
        Overview { total: report.total, passed: report.passed, failed: report.failed, never_run, missing_screenshots, oldest_ms, newest_ms }
    }

    /// 把几个集合各自的概况合在一起（桌面端多选集合时实时汇总）
    pub fn combine(parts: &[&Overview]) -> Overview {
        let nonempty = parts.iter().filter(|p| p.total > 0);
        Overview {
            total: parts.iter().map(|p| p.total).sum(),
            passed: parts.iter().map(|p| p.passed).sum(),
            failed: parts.iter().map(|p| p.failed).sum(),
            never_run: parts.iter().flat_map(|p| p.never_run.iter().cloned()).collect(),
            missing_screenshots: parts.iter().map(|p| p.missing_screenshots).sum(),
            oldest_ms: nonempty.clone().map(|p| p.oldest_ms).min().unwrap_or(0),
            newest_ms: nonempty.map(|p| p.newest_ms).max().unwrap_or(0),
        }
    }

    /// 一行摘要：「42 个用例：40 通过 · 2 失败」
    pub fn headline(&self) -> String {
        if self.failed == 0 {
            format!("{} 个用例：全部通过", self.total)
        } else {
            format!("{} 个用例：{} 通过 · {} 失败", self.total, self.passed, self.failed)
        }
    }

    /// 需要提醒的缺口（没有则为空）
    pub fn notes(&self) -> Vec<String> {
        let mut out = vec![];
        if !self.never_run.is_empty() {
            out.push(format!("还有 {} 个用例没有运行结果，不在报告里（先运行再导出）", self.never_run.len()));
        }
        // 逐个集合分别跑的结果拼在一起时，各用例可能产生于不同时间：失败的可能是早先的旧结果
        if self.newest_ms.saturating_sub(self.oldest_ms) > SPREAD_MS {
            out.push(format!(
                "各用例的结果产生于不同时间（最早 {}，最近 {}），不是同一次运行；要一份当下的结论，先全部重新运行再导出",
                ago(self.oldest_ms),
                ago(self.newest_ms)
            ));
        }
        if self.missing_screenshots > 0 {
            out.push(format!("{} 张截图已被历史清理删除，报告里只保留文字", self.missing_screenshots));
        }
        out
    }
}

/// 最早与最近结果相差超过这个时长就提示（6 小时）
const SPREAD_MS: u64 = 6 * 3_600_000;

/// 「3 小时前」「2 天前」
pub fn ago(ms: u64) -> String {
    let secs = history::now_ms().saturating_sub(ms) / 1000;
    match secs {
        0..=59 => "刚刚".into(),
        60..=3599 => format!("{} 分钟前", secs / 60),
        3600..=86_399 => format!("{} 小时前", secs / 3600),
        _ => format!("{} 天前", secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collection_names_are_normalized_and_listed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("collections/03-作业")).unwrap();
        let root = dir.path();
        for input in ["collections/03-作业", "03-作业", "03-作业/", "./collections/03-作业/"] {
            assert_eq!(normalize_collection(root, input), "collections/03-作业", "{input}");
        }
        assert_eq!(normalize_collection(root, &root.join("collections/03-作业").to_string_lossy()), "collections/03-作业");
        assert_eq!(normalize_collection(root, "collections/不存在"), "collections/不存在");
        // 集合 = collections/ 下的第一级目录；根级用例不属于任何集合
        assert_eq!(top_dir("collections/03-作业/01-a.json").as_deref(), Some("collections/03-作业"));
        assert_eq!(top_dir("collections/03-作业/sub/01-a.json").as_deref(), Some("collections/03-作业"));
        assert_eq!(top_dir("collections/01-a.json"), None);
    }

    #[test]
    fn multi_collection_scope_names_the_file_and_combines_overviews() {
        let meta = |command: &str| crate::report::RunMeta { source: "history".into(), env: "t".into(), command: command.into(), ..Default::default() };
        let one = RunReport::from_cases(vec![], Some(meta("requestbaby export -e t -c collections/03-作业")));
        let two = RunReport::from_cases(vec![], Some(meta("requestbaby export -e t -c collections/03-作业 -c collections/04-测验")));
        let all = RunReport::from_cases(vec![], Some(meta("requestbaby export -e t")));
        let root = Path::new("/p/demo");
        assert_eq!(file_name(root, &one, Format::Html), "验收报告-demo-03-作业-t.html");
        assert_eq!(file_name(root, &two, Format::Json), "验收报告-demo-2个集合-t.json");
        assert_eq!(file_name(root, &all, Format::Html), "验收报告-demo-t.html");
        assert_eq!(collections_of_command(&two), ["collections/03-作业", "collections/04-测验"]);
        assert!(collections_of_command(&all).is_empty());

        let part = |total, passed, old, new| Overview { total, passed, failed: total - passed, never_run: vec![], missing_screenshots: 1, oldest_ms: old, newest_ms: new };
        let sum = Overview::combine(&[&part(3, 2, 10, 20), &part(2, 2, 5, 30)]);
        assert_eq!((sum.total, sum.passed, sum.failed, sum.missing_screenshots, sum.oldest_ms, sum.newest_ms), (5, 4, 1, 2, 5, 30));
    }
}
