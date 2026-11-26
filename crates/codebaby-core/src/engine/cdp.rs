//! Chrome DevTools 协议（CDP）的最小同步客户端 + 浏览器进程管理。
//!
//! 不依赖 Playwright / Node / WebDriver：直接启动本机 Chrome / Edge / Chromium，
//! 通过 `--remote-debugging-port=0` 拿到调试地址，一条 WebSocket 连接收发 JSON 命令。
//! 也可以连接远程浏览器（Docker / 云浏览器 / 另一台机器上开了调试端口的 Chrome）：
//! `web.endpoint` / CODEBABY_BROWSER_URL 写 `http://主机:9222` 或 `ws(s)://…`。
//! 每个用例一个独立的浏览器上下文（Cookie / 存储互不影响），用完即销毁。
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use super::proxy::{ProxyPolicy, Relay};
use tokio_tungstenite::tungstenite::{self, stream::MaybeTlsStream, Connector, Message, WebSocket};

/// 一条 CDP 事件
#[derive(Debug, Clone)]
pub struct Event {
    pub method: String,
    pub params: Value,
    pub session: Option<String>,
}

pub struct Cdp {
    ws: WebSocket<MaybeTlsStream<TcpStream>>,
    next_id: u64,
    events: VecDeque<Event>,
}

/// Chrome 拒绝非 IP / localhost 的 Host 时的回复
const HOST_REJECTED: &str = "Host header is specified and is not an IP address or localhost";

fn handshake(url: &str, proxy: &ProxyPolicy, hostport: &str, tls: bool, timeout: Duration, host: Option<&str>) -> Result<WebSocket<MaybeTlsStream<TcpStream>>> {
    use tungstenite::client::IntoClientRequest;
    let (h, p) = super::proxy::split_host_port(hostport, if tls { 443 } else { 80 });
    let stream = super::proxy::connect(proxy, &h, p, tls, timeout).with_context(|| format!("连接浏览器调试端口 {hostport} 失败"))?;
    stream.set_nodelay(true).ok();
    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(timeout)).ok();
    let mut req = url.into_client_request().map_err(|e| anyhow!("调试地址不对：{e}"))?;
    if let Some(h) = host {
        let port = hostport.rsplit_once(':').map(|(_, p)| p).unwrap_or_default();
        req.headers_mut().insert("Host", format!("{h}:{port}").parse()?);
    }
    let connector = if tls { Connector::Rustls(super::tls::client_config(false, &[])?) } else { Connector::Plain };
    let (ws, _) = tungstenite::client_tls_with_config(req, stream, None, Some(connector)).map_err(|e| match e {
        tungstenite::HandshakeError::Failure(tungstenite::Error::Http(r)) => {
            let body = r.body().as_ref().map(|b| String::from_utf8_lossy(b).trim().chars().take(200).collect::<String>()).unwrap_or_default();
            anyhow!("浏览器调试连接被拒绝（HTTP {}）{}", r.status().as_u16(), if body.is_empty() { String::new() } else { format!("：{body}") })
        }
        e => anyhow!("浏览器调试连接握手失败：{e}"),
    })?;
    Ok(ws)
}

impl Cdp {
    /// 连接调试地址：`ws://`（本机 / 内网）或 `wss://`（云浏览器，证书走系统校验）
    pub fn connect(url: &str, proxy: &ProxyPolicy, timeout: Duration) -> Result<Self> {
        let (tls, rest) = match url.split_once("://") {
            Some(("ws", r)) => (false, r),
            Some(("wss", r)) => (true, r),
            _ => bail!("调试地址不是 ws:// 或 wss://：{}", redact_url(url)),
        };
        let authority = rest.split(['/', '?']).next().unwrap_or_default();
        let authority = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);
        let hostport = if has_port(authority) { authority.to_string() } else { format!("{authority}:{}", if tls { 443 } else { 80 }) };
        let ws = match handshake(url, proxy, &hostport, tls, timeout, None) {
            // Chrome 只接受 Host 为 IP / localhost 的调试连接：用域名（如 compose 里的服务名）访问时换成 localhost
            Err(e) if format!("{e}").contains(HOST_REJECTED) => handshake(url, proxy, &hostport, tls, timeout, Some("localhost"))?,
            r => r?,
        };
        Ok(Self { ws, next_id: 1, events: VecDeque::new() })
    }

    /// 发送命令并等待结果；等待期间到达的事件进入队列
    pub fn call(&mut self, session: Option<&str>, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }
        self.ws.send(Message::Text(msg.to_string().into())).map_err(|e| anyhow!("浏览器连接已断开（{method}）：{e}"))?;
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                bail!("浏览器 {} ms 内没有响应 {method}", timeout.as_millis());
            }
            let Some(v) = self.read(left)? else { continue };
            if v.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(err) = v.get("error") {
                    let m = err.get("message").and_then(Value::as_str).unwrap_or("未知错误");
                    let data = err.get("data").and_then(Value::as_str).map(|d| format!("（{d}）")).unwrap_or_default();
                    bail!("{method} 失败：{m}{data}");
                }
                return Ok(v.get("result").cloned().unwrap_or(Value::Null));
            }
            self.queue(v);
        }
    }

    /// 读取 wait 时间内到达的事件（没有就立即返回空）
    pub fn pump(&mut self, wait: Duration) -> Result<()> {
        let deadline = Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.read(left.max(Duration::from_millis(1)))? {
                Some(v) => self.queue(v),
                None => return Ok(()),
            }
            if Instant::now() >= deadline {
                return Ok(());
            }
        }
    }

    pub fn take_events(&mut self) -> Vec<Event> {
        self.events.drain(..).collect()
    }

    fn queue(&mut self, v: Value) {
        if let Some(m) = v.get("method").and_then(Value::as_str) {
            // alert / confirm / prompt 会卡住页面（连点击命令都不返回）：立刻确认，事件照常入队留记录
            if m == "Page.javascriptDialogOpening" {
                let id = self.next_id;
                self.next_id += 1;
                let prompt = v["params"].get("defaultPrompt").and_then(Value::as_str).unwrap_or_default();
                let mut msg = json!({ "id": id, "method": "Page.handleJavaScriptDialog", "params": { "accept": true, "promptText": prompt } });
                if let Some(s) = v.get("sessionId") {
                    msg["sessionId"] = s.clone();
                }
                let _ = self.ws.send(Message::Text(msg.to_string().into()));
            }
            self.events.push_back(Event {
                method: m.to_string(),
                params: v.get("params").cloned().unwrap_or(Value::Null),
                session: v.get("sessionId").and_then(Value::as_str).map(String::from),
            });
        }
    }

    /// 读一条消息；超时返回 None
    fn read(&mut self, wait: Duration) -> Result<Option<Value>> {
        let wait = Some(wait.max(Duration::from_millis(1)));
        match self.ws.get_mut() {
            MaybeTlsStream::Plain(s) => s.set_read_timeout(wait).ok(),
            MaybeTlsStream::Rustls(s) => s.get_mut().set_read_timeout(wait).ok(),
            _ => None,
        };
        match self.ws.read() {
            Ok(Message::Text(t)) => Ok(serde_json::from_str(t.as_str()).ok()),
            Ok(Message::Close(_)) => bail!("浏览器关闭了调试连接（可能已崩溃）"),
            Ok(_) => Ok(None),
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => Ok(None),
            Err(e) => bail!("浏览器连接已断开：{e}"),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 浏览器进程
// ---------------------------------------------------------------------------------------------

/// 本机可用的浏览器：环境变量 CODEBABY_BROWSER → 项目 web.browser → 常见安装位置
pub fn find_browser(configured: Option<&str>) -> Result<PathBuf> {
    if let Some(p) = std::env::var("CODEBABY_BROWSER").ok().filter(|s| !s.is_empty()).or(configured.map(String::from)) {
        let pb = PathBuf::from(&p);
        if pb.is_file() {
            return Ok(pb);
        }
        if let Some(found) = which(&p) {
            return Ok(found);
        }
        bail!("找不到浏览器 {p}（CODEBABY_BROWSER / codebaby.json 的 web.browser）");
    }
    for c in browser_candidates() {
        if c.is_file() {
            return Ok(c);
        }
    }
    for name in ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "microsoft-edge", "microsoft-edge-stable", "chrome"] {
        if let Some(p) = which(name) {
            return Ok(p);
        }
    }
    bail!("没有找到 Chrome / Edge / Chromium：安装其一，或用 CODEBABY_BROWSER / codebaby.json 的 web.browser 指定路径")
}

fn browser_candidates() -> Vec<PathBuf> {
    let mut out = vec![];
    if cfg!(target_os = "macos") {
        let apps = [
            "Google Chrome.app/Contents/MacOS/Google Chrome",
            "Chromium.app/Contents/MacOS/Chromium",
            "Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "Brave Browser.app/Contents/MacOS/Brave Browser",
            "Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
        ];
        let home = std::env::var("HOME").map(PathBuf::from).ok();
        for a in apps {
            out.push(Path::new("/Applications").join(a));
            if let Some(h) = &home {
                out.push(h.join("Applications").join(a));
            }
        }
    } else if cfg!(windows) {
        for base in [std::env::var("ProgramFiles").ok(), std::env::var("ProgramFiles(x86)").ok(), std::env::var("LOCALAPPDATA").ok()].into_iter().flatten() {
            let b = PathBuf::from(base);
            out.push(b.join("Google/Chrome/Application/chrome.exe"));
            out.push(b.join("Microsoft/Edge/Application/msedge.exe"));
            out.push(b.join("Chromium/Application/chrome.exe"));
        }
    }
    out
}

pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for ext in if cfg!(windows) { &["", ".exe"][..] } else { &[""][..] } {
            let p = dir.join(format!("{name}{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// 远程浏览器地址：环境变量 CODEBABY_BROWSER_URL → 项目 web.endpoint；都没有则启动本机浏览器
pub fn endpoint(configured: Option<&str>) -> Option<String> {
    std::env::var("CODEBABY_BROWSER_URL").ok().or(configured.map(String::from)).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// 按项目配置打开浏览器：配了远程地址就连接远程，否则启动本机浏览器
pub fn open_browser(cfg: &crate::config::ui::WebConfig, proxy: &ProxyPolicy, headed: bool, timeout: Duration) -> Result<Browser> {
    match endpoint(cfg.endpoint.as_deref()) {
        Some(ep) => Browser::connect_remote(&ep, proxy, headed, timeout),
        None => Browser::launch(&find_browser(cfg.browser.as_deref())?, proxy, headed, timeout),
    }
}

/// 地址里的查询串常带令牌（云浏览器 `?token=…`）：显示 / 记录时去掉
pub fn redact_url(url: &str) -> String {
    let (base, query) = url.split_once('?').unwrap_or((url, ""));
    let base = match base.split_once("://") {
        Some((scheme, rest)) => match rest.split_once('/') {
            Some((auth, path)) => format!("{scheme}://{}/{path}", auth.rsplit_once('@').map(|(_, h)| h).unwrap_or(auth)),
            None => format!("{scheme}://{}", rest.rsplit_once('@').map(|(_, h)| h).unwrap_or(rest)),
        },
        None => base.to_string(),
    };
    if query.is_empty() { base } else { format!("{base}?…") }
}

fn has_port(authority: &str) -> bool {
    // [::1]:9222 / host:9222；裸 IPv6 [::1] 没有端口
    match authority.rsplit_once(':') {
        Some((h, p)) => !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) && (!h.contains(':') || h.ends_with(']')),
        None => false,
    }
}

/// 把远程地址解析成浏览器级 WebSocket 调试地址。
///
/// - `ws://` / `wss://`：原样使用（云浏览器通常直接给这种地址，令牌在查询串里）
/// - `http(s)://主机:端口[/前缀]` 或 `主机:端口`：读 `/json/version` 里的 webSocketDebuggerUrl，
///   并把其中的主机端口换回远程地址（容器里的 Chrome 返回的是 127.0.0.1 / localhost）
pub fn resolve_endpoint(ep: &str, proxy: &ProxyPolicy, timeout: Duration) -> Result<String> {
    if ep.starts_with("ws://") || ep.starts_with("wss://") {
        return Ok(ep.to_string());
    }
    let base = if ep.starts_with("http://") || ep.starts_with("https://") { ep.to_string() } else { format!("http://{ep}") };
    let (base, query) = base.split_once('?').map(|(b, q)| (b.to_string(), q.to_string())).unwrap_or((base.clone(), String::new()));
    let url = format!("{}/json/version{}", base.trim_end_matches('/'), if query.is_empty() { String::new() } else { format!("?{query}") });
    let mut body = http_get(&url, proxy, None, timeout)?;
    // Chrome 只接受 Host 为 IP / localhost 的调试请求（防 DNS 重绑定）：用域名访问时换成 localhost 再试
    if body.contains(HOST_REJECTED) {
        body = http_get(&url, proxy, Some("localhost"), timeout)?;
    }
    let v: Value = serde_json::from_str(&body).map_err(|_| anyhow!("{} 不是浏览器调试端口（/json/version 没有返回 JSON）：{}", redact_url(ep), body.trim().chars().take(200).collect::<String>()))?;
    let ws = v.get("webSocketDebuggerUrl").and_then(Value::as_str).ok_or_else(|| anyhow!("{} 的 /json/version 里没有 webSocketDebuggerUrl", redact_url(ep)))?;
    Ok(rewrite_ws(&base, &query, ws))
}

/// ws 地址的主机端口换成 base 的，base 的路径前缀与查询串保留（反向代理 / 令牌）
fn rewrite_ws(base: &str, query: &str, ws: &str) -> String {
    let (scheme, rest) = base.split_once("://").unwrap_or(("http", base));
    let (authority, prefix) = rest.split_once('/').map(|(a, p)| (a, format!("/{}", p.trim_end_matches('/')))).unwrap_or((rest, String::new()));
    let prefix = if prefix == "/" { String::new() } else { prefix };
    let ws_path = ws.split_once("://").map(|(_, r)| r).and_then(|r| r.find('/').map(|i| &r[i..])).unwrap_or("/");
    let ws_scheme = if scheme == "https" { "wss" } else { "ws" };
    let (path, ws_query) = ws_path.split_once('?').unwrap_or((ws_path, ""));
    let q: Vec<&str> = [ws_query, query].into_iter().filter(|q| !q.is_empty()).collect();
    format!("{ws_scheme}://{authority}{prefix}{path}{}", if q.is_empty() { String::new() } else { format!("?{}", q.join("&")) })
}

fn http_get(url: &str, proxy: &ProxyPolicy, host: Option<&str>, timeout: Duration) -> Result<String> {
    let mut easy = curl::easy::Easy::new();
    easy.url(url)?;
    easy.connect_timeout(timeout)?;
    easy.timeout(timeout)?;
    // 与用例请求、随后的 WebSocket 连接同一套代理规则
    match proxy.route_url(url) {
        Some(p) => {
            easy.proxy(&p.url())?;
            easy.noproxy(&p.curl_no_proxy())?;
        }
        None => easy.proxy("")?,
    }
    if let Some(h) = host {
        let mut list = curl::easy::List::new();
        list.append(&format!("Host: {h}"))?;
        easy.http_headers(list)?;
    }
    let mut body = Vec::new();
    {
        let mut t = easy.transfer();
        t.write_function(|d| {
            body.extend_from_slice(d);
            Ok(d.len())
        })?;
        t.perform().map_err(|e| anyhow!("连接远程浏览器 {} 失败：{}", redact_url(url), e.description()))?;
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// 一个浏览器：本机启动的（drop 时关闭并删除临时用户目录），或连接的远程浏览器（drop 时只断开连接）
pub struct Browser {
    child: Option<Child>,
    pub cdp: Cdp,
    user_dir: Option<PathBuf>,
    /// Chrome/157.0.xxxx
    pub product: String,
    pub user_agent: String,
    pub headed: bool,
    pub path: PathBuf,
    /// 远程浏览器地址（已去掉令牌，用于显示）；本机浏览器为 None
    pub remote: Option<String>,
    /// 打开时的代理规则（规则变了就不复用）
    pub proxy: ProxyPolicy,
    /// 本机浏览器经代理时的本地中转（随浏览器关闭）
    relay: Option<Relay>,
}

impl Browser {
    /// 连接远程浏览器；不关闭它，每个用例照样用独立的浏览器上下文
    pub fn connect_remote(ep: &str, proxy: &ProxyPolicy, headed: bool, timeout: Duration) -> Result<Self> {
        let ws = resolve_endpoint(ep, proxy, timeout)?;
        let mut cdp = Cdp::connect(&ws, proxy, timeout).with_context(|| format!("连接远程浏览器 {} 失败", redact_url(ep)))?;
        let v = cdp.call(None, "Browser.getVersion", json!({}), timeout).unwrap_or_default();
        let product = v.get("product").and_then(Value::as_str).unwrap_or_default().to_string();
        let user_agent = v.get("userAgent").and_then(Value::as_str).unwrap_or_default().to_string();
        Ok(Self { child: None, cdp, user_dir: None, product, user_agent, headed, path: PathBuf::new(), remote: Some(redact_url(ep)), proxy: proxy.clone(), relay: None })
    }

    /// 本机浏览器的模式说明 / 远程浏览器的地址
    pub fn mode(&self) -> String {
        match &self.remote {
            Some(ep) => format!("远程（{ep}）"),
            None if self.headed => "有界面".into(),
            None => "无界面（headless）".into(),
        }
    }

    /// 页面流量经哪个代理（运行环境里记录）；直连为 None
    pub fn proxy_note(&self) -> Option<String> {
        match (&self.remote, &self.proxy) {
            (Some(_), ProxyPolicy::On(p)) => Some(format!("调试连接经 {}（页面流量由远程浏览器自己的网络决定）", p.display())),
            (Some(_), _) => None,
            (None, ProxyPolicy::Off) => Some("已禁用（直连）".into()),
            (None, p) => self.relay.as_ref().map(|_| p.describe()),
        }
    }

    pub fn launch(path: &Path, proxy: &ProxyPolicy, headed: bool, timeout: Duration) -> Result<Self> {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        let user_dir = std::env::temp_dir().join(format!("codebaby-browser-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&user_dir)?;
        let mut cmd = Command::new(path);
        cmd.args([
            "--remote-debugging-port=0",
            &format!("--user-data-dir={}", user_dir.display()),
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-background-networking",
            "--disable-component-update",
            "--disable-default-apps",
            "--disable-extensions",
            "--disable-sync",
            "--disable-features=Translate,MediaRouter,OptimizationHints",
            "--disable-popup-blocking",
            "--disable-backgrounding-occluded-windows",
            "--disable-renderer-backgrounding",
            "--metrics-recording-only",
            "--password-store=basic",
            "--use-mock-keychain",
            "--hide-scrollbars",
            "--mute-audio",
            "--force-color-profile=srgb",
        ]);
        if !headed {
            cmd.arg("--headless=new");
        }
        // 代理：浏览器只认不带认证的 SOCKS5，给它一个本地中转，出口按项目规则直连或经代理（含账号密码）；
        // 禁用时明确直连，不用系统代理设置
        let relay = match proxy {
            ProxyPolicy::Off => {
                cmd.arg("--no-proxy-server");
                None
            }
            p if p.route("example.com", true).is_some() || p.route("example.com", false).is_some() => {
                let r = Relay::start(p.clone())?;
                cmd.arg(format!("--proxy-server=socks5://{}", r.addr));
                // WebRTC 的 UDP 不经 SOCKS：禁止它绕开代理
                cmd.arg("--force-webrtc-ip-handling-policy=disable_non_proxied_udp");
                Some(r)
            }
            _ => None,
        };
        cmd.arg("about:blank");
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
        let mut child = cmd.spawn().with_context(|| format!("启动浏览器失败：{}", path.display()))?;
        // 调试地址打印在 stderr；之后继续读走 stderr，避免管道写满卡住浏览器
        let stderr = child.stderr.take().ok_or_else(|| anyhow!("无法读取浏览器输出"))?;
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let tail: Arc<Mutex<Vec<String>>> = Arc::default();
        let tail2 = tail.clone();
        std::thread::spawn(move || {
            let mut sent = false;
            for line in BufReader::new(stderr).lines().map_while(|l| l.ok()) {
                if !sent {
                    if let Some(i) = line.find("ws://") {
                        let _ = tx.send(line[i..].trim().to_string());
                        sent = true;
                        continue;
                    }
                    let mut t = tail2.lock().unwrap();
                    t.push(line);
                    if t.len() > 20 {
                        t.remove(0);
                    }
                }
            }
        });
        let ws_url = match rx.recv_timeout(timeout) {
            Ok(u) => u,
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_dir_all(&user_dir);
                let tail = tail.lock().unwrap().join("\n");
                bail!("浏览器 {} 秒内没有就绪：{}\n{tail}", timeout.as_secs(), path.display());
            }
        };
        let mut cdp = match Cdp::connect(&ws_url, &ProxyPolicy::Off, timeout) {
            Ok(c) => c,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_dir_all(&user_dir);
                return Err(e);
            }
        };
        let v = cdp.call(None, "Browser.getVersion", json!({}), timeout).unwrap_or_default();
        let product = v.get("product").and_then(Value::as_str).unwrap_or_default().to_string();
        let user_agent = v.get("userAgent").and_then(Value::as_str).unwrap_or_default().to_string();
        Ok(Self { child: Some(child), cdp, user_dir: Some(user_dir), product, user_agent, headed, path: path.to_path_buf(), remote: None, proxy: proxy.clone(), relay })
    }

    /// 本机浏览器进程还在（远程浏览器以调试连接能否应答为准）
    pub fn alive(&mut self) -> bool {
        match &mut self.child {
            Some(c) => matches!(c.try_wait(), Ok(None)),
            None => true,
        }
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let Some(child) = &mut self.child else {
            // 远程浏览器不归我们管：只断开连接（各用例的上下文已在 WebPage drop 时销毁）
            let _ = self.cdp.ws.close(None);
            let _ = self.cdp.ws.flush();
            return;
        };
        let _ = self.cdp.call(None, "Browser.close", json!({}), Duration::from_millis(1500));
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if !matches!(child.try_wait(), Ok(None)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        let _ = child.wait();
        if let Some(d) = &self.user_dir {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_ws_address_keeps_endpoint_host_prefix_and_token() {
        let ws = "ws://127.0.0.1:9222/devtools/browser/abc";
        assert_eq!(rewrite_ws("http://10.0.0.5:9222", "", ws), "ws://10.0.0.5:9222/devtools/browser/abc");
        assert_eq!(rewrite_ws("https://chrome.example.com/", "", ws), "wss://chrome.example.com/devtools/browser/abc");
        assert_eq!(rewrite_ws("https://gw.example.com/chrome/", "token=x", ws), "wss://gw.example.com/chrome/devtools/browser/abc?token=x");
        assert_eq!(rewrite_ws("http://[::1]:9222", "", "ws://localhost:9222/devtools/browser/abc?a=1"), "ws://[::1]:9222/devtools/browser/abc?a=1");
    }

    #[test]
    fn remote_address_display_hides_token_and_userinfo() {
        assert_eq!(redact_url("wss://cloud.example.com?token=secret"), "wss://cloud.example.com?…");
        assert_eq!(redact_url("wss://u:p@cloud.example.com/chrome?token=s"), "wss://cloud.example.com/chrome?…");
        assert_eq!(redact_url("http://10.0.0.5:9222"), "http://10.0.0.5:9222");
    }

    #[test]
    fn port_detection() {
        assert!(has_port("host:9222"));
        assert!(has_port("[::1]:9222"));
        assert!(!has_port("host"));
        assert!(!has_port("[::1]"));
    }
}
