//! libcurl 执行引擎（curl crate 官方 binding，static-curl 静态编译）：
//! - 单二进制内嵌 libcurl，零外部进程/动态库依赖
//! - 行为与系统 curl 一致（TLS/代理环境变量/HTTP2 自动协商）
//! - 全内存执行（body 不落盘、无临时文件）
use super::{CurlInvocation, ResolvedRequest, Response, Transport};
use anyhow::{Context, Result};
use std::time::Duration;

#[derive(Debug, Clone, Copy, Default)]
pub struct CurlTransport;

impl CurlTransport {
    pub fn new() -> Self {
        Self
    }

    /// 生成 curl 命令行展示（dry-run / -v 复现命令；纯字符串，不执行）
    pub fn compile(&self, req: &ResolvedRequest) -> Result<CurlInvocation> {
        let mut argv: Vec<String> = vec![
            "curl".into(),
            "-sS".into(),
            "--max-time".into(),
            format!("{}", req.timeout_ms.div_ceil(1000)),
        ];
        if req.retry > 0 {
            argv.push("--retry".into());
            argv.push(req.retry.to_string());
        }
        if req.insecure {
            argv.push("-k".into());
        }
        if req.follow_redirects {
            argv.push("-L".into());
        }
        argv.push("-X".into());
        argv.push(req.method.clone());
        argv.push(req.url.clone());
        for (k, v) in &req.headers {
            argv.push("-H".into());
            argv.push(format!("{k}: {v}"));
        }
        Ok(CurlInvocation {
            argv,
            body: req.body.clone(),
        })
    }
}

impl Transport for CurlTransport {
    /// 单次 libcurl 执行（传输层错误按 retry 配置重试）
    fn execute(&self, req: &ResolvedRequest) -> Result<Response> {
        let attempts = req.retry + 1;
        let mut last_err = None;
        for _ in 0..attempts {
            match perform_once(req) {
                Ok(resp) => return Ok(resp),
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("未知传输错误")))
    }
}

fn perform_once(req: &ResolvedRequest) -> Result<Response> {
    let mut easy = curl::easy::Easy::new();
    easy.url(&req.url).with_context(|| format!("非法 URL: {}", req.url))?;
    easy.timeout(Duration::from_millis(req.timeout_ms.max(1)))?;
    easy.connect_timeout(Duration::from_secs(10))?;
    if req.follow_redirects {
        easy.follow_location(true)?;
        easy.max_redirections(10)?;
    }
    if req.insecure {
        easy.ssl_verify_peer(false)?;
        easy.ssl_verify_host(false)?;
    }

    // 方法与 body：POST/PUT/PATCH 走 post_fields；其余（含 GET+body）custom_request 显式指定
    let has_body = req.body.as_deref().map(|b| !b.is_empty()).unwrap_or(false);
    if has_body {
        easy.custom_request(&req.method)?;
        easy.post(true)?;
        easy.post_fields_copy(req.body.as_deref().unwrap().as_bytes())?;
    } else {
        match req.method.as_str() {
            "POST" => easy.post(true)?,
            "GET" => easy.get(true)?,
            "HEAD" => easy.custom_request("HEAD")?,
            other => easy.custom_request(other)?,
        }
    }

    let mut header_list = curl::easy::List::new();
    for (k, v) in &req.headers {
        header_list.append(&format!("{k}: {v}"))?;
    }
    easy.http_headers(header_list)?;

    let mut body_buf: Vec<u8> = vec![];
    let mut header_lines: Vec<String> = vec![];
    {
        let mut transfer = easy.transfer();
        transfer
            .write_function(|data| {
                body_buf.extend_from_slice(data);
                Ok(data.len())
            })
            .context("注册 body 回调失败")?;
        transfer
            .header_function(|data| {
                header_lines.push(String::from_utf8_lossy(data).trim_end().to_string());
                true
            })
            .context("注册 header 回调失败")?;
        transfer.perform().map_err(|e| anyhow::anyhow!("curl: {e}"))?;
    }

    let status = easy.response_code()? as u16;
    let time_ms = easy.total_time()?.as_millis() as u64;
    let size = body_buf.len() as u64;
    let headers = parse_headers(&header_lines);
    Ok(Response {
        status,
        headers,
        body: body_buf,
        time_ms,
        size,
        transport_error: None,
    })
}

/// 解析 header 行（跳过状态行与重定向多段分隔）
fn parse_headers(lines: &[String]) -> Vec<(String, String)> {
    let mut out = vec![];
    for line in lines {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with("HTTP/") {
            continue;
        }
        if let Some(idx) = line.find(':') {
            let k = line[..idx].trim().to_string();
            let v = line[idx + 1..].trim().to_string();
            if !k.is_empty() {
                out.push((k, v));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_headers_skips_status_lines() {
        let lines = vec![
            "HTTP/1.1 200 OK".into(),
            "Content-Type: application/json".into(),
            "".into(),
            "HTTP/1.1 404 Not Found".into(),
            "Set-Cookie: a=1".into(),
        ];
        let h = parse_headers(&lines);
        assert_eq!(h.len(), 2);
        assert_eq!(h[0], ("Content-Type".into(), "application/json".into()));
    }

    #[test]
    fn compile_get_has_no_body() {
        let t = CurlTransport::new();
        let req = ResolvedRequest {
            method: "GET".into(),
            url: "http://x/y".into(),
            headers: vec![],
            body: None,
            timeout_ms: 1500,
            retry: 0,
            insecure: false,
            follow_redirects: false,
        };
        let inv = t.compile(&req).unwrap();
        assert!(inv.argv.contains(&"--max-time".to_string()));
        assert!(inv.argv.contains(&"2".to_string()));
    }
}
