//! 通知：界面用例里 `notify` 步骤（含 `onFail`）做的事。
//!
//! 只做三件固定的事：弹本机通知、追加到 `.requestbaby/alerts.log`、POST 到 webhook。
//! 不运行用例或项目里写的任何命令——用例文件来自别人的仓库时，打开并运行它也不会在本机执行代码。

use crate::engine::{CurlTransport, ResolvedRequest, Transport};
use std::path::Path;
use std::time::{Duration, Instant};

/// 通知内容最长保留的字符数（本机通知和日志都只留开头）
const MAX_CHARS: usize = 500;

pub struct NotifyOutcome {
    /// 做成了哪些，如 ["本机通知", "alerts.log", "webhook"]
    pub done: Vec<&'static str>,
    /// 失败的原因（日志写不进去、webhook 非 2xx 等）；本机通知弹不出来不算失败（无界面的机器本来就没有）
    pub error: Option<String>,
}

pub fn send(message: &str, webhook: Option<&str>, project_root: &Path, proxy: &crate::engine::proxy::ProxyPolicy) -> NotifyOutcome {
    let msg: String = message.trim().chars().take(MAX_CHARS).collect();
    let mut out = NotifyOutcome { done: vec![], error: None };
    if msg.is_empty() {
        out.error = Some("notify 要写通知内容".into());
        return out;
    }
    if desktop_notification(&msg) {
        out.done.push("本机通知");
    }
    match append_log(project_root, &msg) {
        Ok(()) => out.done.push("alerts.log"),
        Err(e) => out.error = Some(format!("写不进 .requestbaby/alerts.log：{e}")),
    }
    if let Some(url) = webhook.map(str::trim).filter(|u| !u.is_empty()) {
        match post_webhook(url, &msg, proxy) {
            Ok(()) => out.done.push("webhook"),
            Err(e) => out.error = Some(e),
        }
    }
    out
}

fn append_log(root: &Path, msg: &str) -> std::io::Result<()> {
    use std::io::Write;
    let dir = root.join(".requestbaby");
    std::fs::create_dir_all(&dir)?;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("alerts.log"))?;
    writeln!(f, "{} {}", local_time(), msg.replace('\n', " "))
}

/// 本地时间 `2026-11-25 23:41:07`（拿不到时区信息时退回 UTC 秒数）
fn local_time() -> String {
    #[cfg(unix)]
    {
        let secs = (crate::history::now_ms() / 1000) as libc::time_t;
        // SAFETY: localtime_r 只写入我们提供的 tm
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        if !unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
            return format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec);
        }
    }
    format!("{}", crate::history::now_ms() / 1000)
}

/// 系统通知：macOS 用 osascript，Linux 用 notify-send（没装就算了）。内容作为参数传入，不拼进脚本
fn desktop_notification(msg: &str) -> bool {
    use std::process::{Command, Stdio};
    let mut c = if cfg!(target_os = "macos") {
        let mut c = Command::new("osascript");
        c.args(["-e", "on run argv", "-e", "display notification (item 1 of argv) with title \"requestbaby\"", "-e", "end run", msg]);
        c
    } else if cfg!(target_os = "linux") {
        let mut c = Command::new("notify-send");
        c.args(["requestbaby", msg]);
        c
    } else {
        return false;
    };
    let Ok(mut child) = c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else { return false };
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(s)) => return s.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(30)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

fn post_webhook(url: &str, msg: &str, proxy: &crate::engine::proxy::ProxyPolicy) -> Result<(), String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!("webhook 要以 http:// 或 https:// 开头：{url}"));
    }
    let body = serde_json::json!({ "text": msg }).to_string();
    let req = ResolvedRequest {
        method: "POST".into(),
        url: url.to_string(),
        headers: vec![("Content-Type".into(), "application/json".into())],
        body: Some(body),
        timeout_ms: 10_000,
        proxy: proxy.clone(),
        ..Default::default()
    };
    match CurlTransport::new().execute(&req) {
        Ok(r) if (200..300).contains(&r.status) => Ok(()),
        Ok(r) if r.status > 0 => Err(format!("webhook 返回 HTTP {}", r.status)),
        Ok(r) => Err(format!("webhook 发送失败：{}", r.transport_error.unwrap_or_else(|| "连不上".into()))),
        Err(e) => Err(format!("webhook 发送失败：{e:#}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_the_log_and_rejects_empty_text_and_bad_webhooks() {
        let dir = tempfile::tempdir().unwrap();
        let proxy = crate::engine::proxy::ProxyPolicy::default();
        let r = send("11-28 有号\n第二行", None, dir.path(), &proxy);
        assert!(r.error.is_none() && r.done.contains(&"alerts.log"), "{:?}", r.done);
        let log = std::fs::read_to_string(dir.path().join(".requestbaby/alerts.log")).unwrap();
        assert!(log.trim_end().ends_with("11-28 有号 第二行") && log.lines().count() == 1, "{log}");
        assert!(send("  ", None, dir.path(), &proxy).error.unwrap().contains("通知内容"));
        assert!(send("x", Some("ftp://a/b"), dir.path(), &proxy).error.unwrap().contains("http"));
    }
}
