use super::{CurlInvocation, ResolvedRequest, Response, Transport};
use anyhow::{Context, Result};
use serde_json::Value;
use std::process::Command;

/// curl 子进程传输实现：
/// - 参数数组直接传递，不经 shell（无注入风险）
/// - 元数据走 `-w %{json}`（curl >= 7.75）
/// - 响应头 `-D <file>`、响应体 `-o <file>`、body 临时文件 `--data-binary @file`
#[derive(Debug, Default, Clone, Copy)]
pub struct CurlTransport;

impl CurlTransport {
    pub fn new() -> Self {
        Self
    }
}

impl Transport for CurlTransport {
    fn execute(&self, req: &ResolvedRequest) -> Result<Response> {
        let inv = self.compile(req)?;
        self.run(&inv)
    }
}

impl CurlTransport {
    pub fn compile(&self, req: &ResolvedRequest) -> Result<CurlInvocation> {
        let mut argv: Vec<String> = vec![
            "curl".into(),
            "-sS".into(),
            "--max-time".into(),
            format!("{}", (req.timeout_ms + 999) / 1000),
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

        let mut body_file = None;
        if let Some(body) = &req.body {
            let mut f = tempfile::Builder::new()
                .prefix("requestbaby-body-")
                .suffix(".bin")
                .tempfile()
                .context("创建 body 临时文件失败")?;
            std::io::Write::write_all(&mut f, body.as_bytes())?;
            argv.push("--data-binary".into());
            argv.push(format!("@{}", f.path().display()));
            body_file = Some(f.into_temp_path());
        }

        Ok(CurlInvocation { argv, body: body_file })
    }

    pub fn run(&self, inv: &CurlInvocation) -> Result<Response> {
        let mut argv = inv.argv.clone();

        let hdr_file = tempfile::Builder::new()
            .prefix("requestbaby-hdr-")
            .tempfile()
            .context("创建响应头临时文件失败")?;
        let body_file = tempfile::Builder::new()
            .prefix("requestbaby-resp-")
            .tempfile()
            .context("创建响应体临时文件失败")?;
        argv.push("-D".into());
        argv.push(hdr_file.path().display().to_string());
        argv.push("-o".into());
        argv.push(body_file.path().display().to_string());
        argv.push("-w".into());
        argv.push("%{json}".into());

        // argv[0] 是 "curl"，Command::new 直接用程序名
        let prog = argv.remove(0);
        let output = Command::new(&prog)
            .args(&argv)
            .output()
            .with_context(|| format!("启动 curl 失败: {prog}（是否已安装？）"))?;

        let body = std::fs::read(body_file.path()).unwrap_or_default();
        let headers = parse_header_file(&std::fs::read_to_string(hdr_file.path()).unwrap_or_default());

        let mut resp = Response {
            headers,
            body,
            ..Default::default()
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            resp.transport_error = Some(format!(
                "curl exit {}: {}",
                output.status.code().unwrap_or(-1),
                stderr
            ));
            // 尽力解析已获得的元数据
            let _ = parse_meta(&String::from_utf8_lossy(&output.stdout), &mut resp);
            return Ok(resp);
        }

        parse_meta(&String::from_utf8_lossy(&output.stdout), &mut resp)?;
        Ok(resp)
    }
}

/// 解析 -w '%{json}' 元数据
fn parse_meta(stdout: &str, resp: &mut Response) -> Result<()> {
    let meta: Value = serde_json::from_str(stdout.trim())
        .with_context(|| format!("解析 curl %{{json}} 输出失败: {stdout:?}"))?;
    resp.status = meta.get("http_code").and_then(Value::as_u64).unwrap_or(0) as u16;
    resp.time_ms = (meta
        .get("time_total")
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
        * 1000.0) as u64;
    resp.size = meta.get("size_download").and_then(Value::as_u64).unwrap_or(0);
    Ok(())
}

/// 解析 -D 响应头文件（容忍多段：跟随重定向时会有多组状态行）
fn parse_header_file(content: &str) -> Vec<(String, String)> {
    let mut out = vec![];
    for line in content.lines() {
        let line = line.trim_end_matches('\r');
        // 跳过状态行
        if line.starts_with("HTTP/") {
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
        let raw = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nSet-Cookie: a=1\r\nSet-Cookie: b=2\r\n\r\nbody";
        let h = parse_header_file(raw);
        assert_eq!(h.len(), 3);
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
        assert!(!inv.argv.iter().any(|a| a.starts_with("--data")));
    }
}
