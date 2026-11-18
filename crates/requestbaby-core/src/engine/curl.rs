//! libcurl 执行引擎（curl crate 官方 binding，static-curl 静态编译）：
//! - 单二进制内嵌 libcurl，零外部进程/动态库依赖
//! - 行为与系统 curl 一致（TLS/代理环境变量/HTTP2 自动协商）
//! - 全内存执行（body 不落盘、无临时文件）
use super::proxy::ProxyPolicy;
use super::{CurlInvocation, PartValue, ResolvedRequest, Response, Transport};
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
        // 只写明确配置的代理；跟随系统时不写（各人机器上的系统代理不同，命令要能原样分享）
        match (&req.proxy, req.proxy.route_url(&req.url)) {
            (ProxyPolicy::On(_), Some(p)) => {
                // 账号密码不进命令（命令会被复制、贴进对话）
                let shown = if p.user.is_some() { super::proxy::redact(&p.url()) } else { p.url() };
                argv.extend(["-x".into(), shown, "--noproxy".into(), p.curl_no_proxy()]);
            }
            (ProxyPolicy::Off, _) => argv.extend(["--noproxy".into(), "*".into()]),
            _ => {}
        }
        argv.push("-X".into());
        argv.push(req.method.clone());
        argv.push(req.url.clone());
        for (k, v) in &req.headers {
            argv.push("-H".into());
            argv.push(format!("{k}: {v}"));
        }
        for part in &req.multipart {
            argv.push("-F".into());
            argv.push(match &part.value {
                PartValue::Text(v) => format!("{}={v}", part.name),
                PartValue::File { path, filename, content_type } => {
                    let mut s = format!("{}=@{}", part.name, path.display());
                    if let Some(t) = content_type {
                        s.push_str(&format!(";type={t}"));
                    }
                    if let Some(f) = filename {
                        s.push_str(&format!(";filename={f}"));
                    }
                    s
                }
            });
        }
        if let Some(b) = req.body.as_ref().filter(|b| !b.is_empty() && req.multipart.is_empty()) {
            argv.push("--data-raw".into());
            argv.push(b.clone());
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

thread_local! {
    /// 每线程复用一个 libcurl 句柄：reset 清空选项但保留连接池、DNS 与 TLS 会话缓存，
    /// 同一主机的后续请求不再重新握手（https 下差别明显）
    static EASY: std::cell::RefCell<Option<curl::easy::Easy>> = const { std::cell::RefCell::new(None) };
}

fn perform_once(req: &ResolvedRequest) -> Result<Response> {
    let mut easy = EASY.with(|c| c.borrow_mut().take()).unwrap_or_else(curl::easy::Easy::new);
    easy.reset();
    let out = perform_with(&mut easy, req);
    EASY.with(|c| *c.borrow_mut() = Some(easy));
    out
}

fn perform_with(easy: &mut curl::easy::Easy, req: &ResolvedRequest) -> Result<Response> {
    easy.url(&req.url).with_context(|| format!("非法 URL: {}", req.url))?;
    easy.timeout(Duration::from_millis(req.timeout_ms.max(1)))?;
    easy.connect_timeout(Duration::from_secs(10))?;
    // 代理由 requestbaby 统一判定（跟随系统时含 macOS 系统代理设置），不让 libcurl 再读环境变量
    match req.proxy.route_url(&req.url) {
        Some(p) => {
            easy.proxy(&p.url())?;
            easy.noproxy(&p.curl_no_proxy())?;
        }
        // 空串 = 明确不用代理
        None => easy.proxy("")?,
    }
    if req.follow_redirects {
        easy.follow_location(true)?;
        easy.max_redirections(10)?;
    }
    if req.insecure {
        easy.ssl_verify_peer(false)?;
        easy.ssl_verify_host(false)?;
    } else {
        // 信任系统证书库（含用户在钥匙串中添加的企业 / 自签 CA），与浏览器、系统 curl 一致
        easy.ssl_options(curl::easy::SslOpt::new().native_ca(true))?;
    }

    // 方法与 body：multipart → httppost；有 body 走 post_fields；其余（含 GET+body）custom_request 显式指定
    let has_body = req.body.as_deref().map(|b| !b.is_empty()).unwrap_or(false);
    if !req.multipart.is_empty() {
        let mut form = curl::easy::Form::new();
        for part in &req.multipart {
            let mut p = form.part(&part.name);
            match &part.value {
                PartValue::Text(v) => {
                    p.contents(v.as_bytes());
                }
                PartValue::File { path, filename, content_type } => {
                    if !path.is_file() {
                        anyhow::bail!("上传文件不存在: {}", path.display());
                    }
                    p.file(path);
                    if let Some(f) = filename {
                        p.filename(f);
                    }
                    if let Some(t) = content_type {
                        p.content_type(t);
                    }
                }
            }
            p.add().map_err(|e| anyhow::anyhow!("multipart 字段 {} 无效: {e}", part.name))?;
        }
        easy.httppost(form)?;
        if req.method != "POST" {
            easy.custom_request(&req.method)?;
        }
    } else if has_body {
        easy.custom_request(&req.method)?;
        easy.post(true)?;
        easy.post_fields_copy(req.body.as_deref().unwrap().as_bytes())?;
    } else {
        match req.method.as_str() {
            "POST" => easy.post(true)?,
            "GET" => easy.get(true)?,
            "HEAD" => easy.nobody(true)?,
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
        transfer
            .perform()
            .map_err(|e| anyhow::anyhow!("{}", explain_with_proxy(req, e.code(), e.description())))?;
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

/// 经代理时，代理本身的问题（连不上、认证失败）要说是代理，而不是被测服务
fn explain_with_proxy(req: &ResolvedRequest, code: u32, raw: &str) -> String {
    let Some(p) = req.proxy.route_url(&req.url) else { return explain_transport_error(code, raw, &req.url, req.timeout_ms, req.insecure) };
    let p = &p;
    let shown = p.display();
    match code {
        5 => format!("找不到代理 {shown} 的主机：检查代理地址（curl {code}：{raw}）"),
        97 => format!("和代理 {shown} 握手失败：账号密码不对，或代理协议写错（socks5 / socks5h / http）（curl {code}：{raw}）"),
        // curl 不区分连不上代理还是代理连不上目标：直接试一下代理
        7 if super::proxy::probe(p, Duration::from_secs(2)).is_err() => {
            format!("连不上代理 {shown}：代理没有运行，或地址 / 端口不对（requestbaby proxy 检查；--no-proxy 临时直连）（curl {code}：{raw}）")
        }
        _ => format!("{}（经代理 {shown}）", explain_transport_error(code, raw, &req.url, req.timeout_ms, req.insecure)),
    }
}

/// 传输层错误 → 人话：先说发生了什么、该查什么，括号里保留 curl 错误码与原文便于排查
pub fn explain_transport_error(code: u32, raw: &str, url: &str, timeout_ms: u64, insecure: bool) -> String {
    let scheme_end = url.find("://").map(|i| i + 3).unwrap_or(0);
    let rest = &url[scheme_end..];
    let host_port = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    let host_port = host_port.rsplit('@').next().unwrap_or(host_port);
    let host = host_port.split(':').next().unwrap_or(host_port);
    let what = match code {
        1 | 3 => format!("URL 写法不对：{url}（变量没有替换？只支持 http / https）"),
        6 => format!("找不到主机 {host}：域名写错了，或当前网络 / VPN 访问不到它；检查环境的 baseUrl"),
        7 => format!("连不上 {host_port}：服务没有启动，或端口 / baseUrl 不对"),
        28 => format!("{timeout_ms} ms 内没有完成：服务太慢或卡住了（需要时调大 timeoutMs）"),
        35 | 60 if !insecure => format!("和 {host} 的 TLS 握手 / 证书校验失败：证书自签、过期或域名不符（测试环境可设 request.insecure: true）"),
        35 => format!("和 {host} 的 TLS 握手失败：端口不是 https，或服务端 TLS 配置有误"),
        47 => "重定向超过 10 次：多半是循环重定向".to_string(),
        52 => format!("{host_port} 没有返回任何内容就断开了：服务可能崩溃了，或协议不对（https 端口用了 http？）"),
        55 | 56 => format!("与 {host_port} 的连接中途断开：服务端重置了连接"),
        _ => "网络请求失败".to_string(),
    };
    format!("{what}（curl {code}：{raw}）")
}

/// 解析 header 行：跟随重定向时取最后一个响应的头，但保留途中各跳的 Set-Cookie（登录重定向常在 302 上种 cookie）
fn parse_headers(lines: &[String]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = vec![];
    for line in lines {
        let line = line.trim_end_matches('\r');
        if line.starts_with("HTTP/") {
            out.retain(|(k, _)| k.eq_ignore_ascii_case("set-cookie"));
            continue;
        }
        if line.is_empty() {
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

    #[test]
    fn transport_errors_read_like_advice() {
        let e = |code, url| explain_transport_error(code, "x", url, 3000, false);
        assert_eq!(e(7, "http://127.0.0.1:18087/api?a=1"), "连不上 127.0.0.1:18087：服务没有启动，或端口 / baseUrl 不对（curl 7：x）");
        assert!(e(6, "https://u:p@api.test.example/v1").starts_with("找不到主机 api.test.example："));
        assert!(e(28, "http://h/x").starts_with("3000 ms 内没有完成"));
        assert!(e(60, "https://h/x").contains("insecure: true"));
        assert!(!explain_transport_error(35, "x", "https://h/x", 1, true).contains("insecure"));
        assert!(e(3, "{{baseUrl}}/x").contains("变量没有替换"));
        assert_eq!(e(99, "http://h"), "网络请求失败（curl 99：x）");
    }

    /// 真实连接一个没有服务的端口：报错直接说「连不上」
    #[test]
    fn refused_connection_is_explained() {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let req = ResolvedRequest { method: "GET".into(), url: format!("http://127.0.0.1:{port}/x"), timeout_ms: 2000, ..Default::default() };
        let err = CurlTransport.execute(&req).unwrap_err().to_string();
        assert!(err.starts_with(&format!("连不上 127.0.0.1:{port}")), "{err}");
    }

    use super::*;

    #[test]
    fn parse_headers_keeps_final_response_and_hop_cookies() {
        let lines = vec![
            "HTTP/1.1 302 Found".into(),
            "Location: /home".into(),
            "Content-Type: text/html".into(),
            "Set-Cookie: sid=1".into(),
            "".into(),
            "HTTP/1.1 200 OK".into(),
            "Content-Type: application/json".into(),
            "Set-Cookie: a=1".into(),
        ];
        let h = parse_headers(&lines);
        assert_eq!(
            h,
            vec![
                ("Set-Cookie".into(), "sid=1".into()),
                ("Content-Type".into(), "application/json".into()),
                ("Set-Cookie".into(), "a=1".into()),
            ]
        );
    }

    #[test]
    fn compile_get_has_no_body() {
        let t = CurlTransport::new();
        let req = ResolvedRequest { method: "GET".into(), url: "http://x/y".into(), timeout_ms: 1500, ..Default::default() };
        let inv = t.compile(&req).unwrap();
        assert!(inv.argv.contains(&"--max-time".to_string()));
        assert!(inv.argv.contains(&"2".to_string()));
        assert!(!inv.argv.contains(&"--data-raw".to_string()));
    }

    /// 「复制为 cURL」粘贴到终端即可执行：URL 中的 & / body 中的引号都被正确转义
    #[test]
    fn shell_command_is_pasteable() {
        let req = ResolvedRequest {
            method: "POST".into(),
            url: "http://x/api?a=1&b=2".into(),
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: Some(r#"{"name":"O'Brien"}"#.into()),
            timeout_ms: 30_000,
            ..Default::default()
        };
        let cmd = CurlTransport::new().compile(&req).unwrap().shell();
        assert_eq!(
            cmd,
            r#"curl -sS --max-time 30 -X POST 'http://x/api?a=1&b=2' -H 'Content-Type: application/json' --data-raw '{"name":"O'\''Brien"}'"#
        );
        let out = std::process::Command::new("sh").args(["-c", &format!("printf '%s\n' {}", cmd.trim_start_matches("curl "))]).output().unwrap();
        let args: Vec<String> = String::from_utf8_lossy(&out.stdout).lines().map(String::from).collect();
        assert!(args.contains(&"http://x/api?a=1&b=2".to_string()) && args.contains(&r#"{"name":"O'Brien"}"#.to_string()), "{args:?}");
    }

    #[test]
    fn multipart_compiles_to_form_flags() {
        let req = ResolvedRequest {
            method: "POST".into(),
            url: "http://x/upload".into(),
            multipart: vec![
                super::super::FormPart { name: "title".into(), value: PartValue::Text("报告".into()) },
                super::super::FormPart {
                    name: "file".into(),
                    value: PartValue::File { path: "/tmp/a.png".into(), filename: None, content_type: Some("image/png".into()) },
                },
            ],
            ..Default::default()
        };
        let inv = CurlTransport::new().compile(&req).unwrap();
        assert!(inv.shell().ends_with("-F 'title=报告' -F 'file=@/tmp/a.png;type=image/png'"), "{}", inv.shell());
    }
}
