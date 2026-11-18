//! WebSocket 引擎：同步会话接口（runner 逐步驱动：发送 → 接收并断言 → 捕获 → 再发送…）。
//! 底层 tokio-tungstenite；wss 与 gRPC 共用 rustls + 系统证书校验。
use super::{tls, Frame};
use anyhow::{anyhow, bail, Result};
use futures_util::{SinkExt, StreamExt};
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type Stream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// 连接参数（变量已替换）
#[derive(Debug, Clone, Default)]
pub struct WsConnect {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub subprotocols: Vec<String>,
    pub timeout_ms: u64,
    pub insecure: bool,
    pub proxy: super::proxy::ProxyPolicy,
}

/// 握手结果：成功 101；被拒时是服务端返回的 HTTP 状态（401 / 403…），此时没有会话
#[derive(Debug, Clone, Default)]
pub struct Handshake {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// 被拒时的响应体
    pub body: String,
}

/// 一次接收的结果
#[derive(Debug, Clone, PartialEq)]
pub enum Recv {
    /// 收到一条数据消息（下标指向 frames）
    Message(usize),
    /// 服务端关闭了连接
    Closed,
    /// 超时没有收到
    Timeout,
}

pub struct WsSession {
    rt: tokio::runtime::Runtime,
    ws: Option<Stream>,
    started: Instant,
    /// 对话记录：发送与收到的消息、关闭帧，按时间顺序
    pub frames: Vec<Frame>,
    /// 服务端关闭：(关闭码, 原因)
    pub closed_by_server: Option<(u16, String)>,
    pub received_bytes: u64,
}

/// ws(s):// 地址；http(s):// 按同一主机换成 ws(s)://
pub fn normalize_url(raw: &str) -> Result<String> {
    let t = raw.trim();
    let lower = t.to_ascii_lowercase();
    if lower.starts_with("ws://") || lower.starts_with("wss://") {
        Ok(t.to_string())
    } else if lower.starts_with("http://") {
        Ok(format!("ws://{}", &t[7..]))
    } else if lower.starts_with("https://") {
        Ok(format!("wss://{}", &t[8..]))
    } else if t.contains("://") {
        bail!("WebSocket 地址 {t} 的协议不支持：写 ws:// 或 wss://")
    } else {
        bail!("WebSocket 地址 {t} 缺少协议：写 ws://host/path 或 wss://host/path")
    }
}

fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let hp = rest.split(['/', '?', '#']).next().unwrap_or_default();
    hp.rsplit('@').next().unwrap_or(hp).to_string()
}

/// 建立连接。握手被拒（非 101）不算错误：返回状态码与响应，会话不可收发
pub fn connect(c: &WsConnect) -> Result<(WsSession, Handshake)> {
    let url = normalize_url(&c.url)?;
    let rt = tls::runtime()?;
    let started = Instant::now();
    let mut req = url.as_str().into_client_request().map_err(|e| anyhow!("WebSocket 地址无效：{e}"))?;
    for (k, v) in &c.headers {
        let name = http::HeaderName::from_bytes(k.trim().as_bytes()).map_err(|_| anyhow!("请求头名 {k} 不合法"))?;
        let value = http::HeaderValue::from_str(v).map_err(|_| anyhow!("请求头 {k} 的值含有不能放进请求头的字符"))?;
        req.headers_mut().append(name, value);
    }
    if !c.subprotocols.is_empty() {
        let v = http::HeaderValue::from_str(&c.subprotocols.join(", ")).map_err(|_| anyhow!("子协议名不合法"))?;
        req.headers_mut().insert("Sec-WebSocket-Protocol", v);
    }
    let connector = if url.to_ascii_lowercase().starts_with("wss://") {
        Some(tokio_tungstenite::Connector::Rustls(tls::client_config(c.insecure, &[])?))
    } else {
        None
    };
    let timeout = Duration::from_millis(c.timeout_ms.max(1));
    let host = host_of(&url);
    let tls_on = connector.is_some();
    let (h, port) = super::proxy::split_host_port(&host, if tls_on { 443 } else { 80 });
    let proxy = c.proxy.clone();
    let res = rt.block_on(async {
        tokio::time::timeout(timeout, async {
            // TCP 自己拨（直连或经代理），WebSocket / TLS 握手在其上进行
            let tcp = super::proxy::connect_async(proxy, h, port, tls_on, timeout).await.map_err(WsError::Io)?;
            tcp.set_nodelay(true).ok();
            tokio_tungstenite::client_async_tls_with_config(req, tcp, None, connector).await
        })
        .await
    });
    let mut session = WsSession { rt, ws: None, started, frames: vec![], closed_by_server: None, received_bytes: 0 };
    match res {
        Err(_) => bail!("{} ms 内没有连上 {host}：服务太慢或卡住了（需要时调大 timeoutMs）", c.timeout_ms),
        Ok(Ok((ws, resp))) => {
            session.ws = Some(ws);
            Ok((session, Handshake { status: resp.status().as_u16(), headers: header_pairs(resp.headers()), body: String::new() }))
        }
        Ok(Err(WsError::Http(resp))) => {
            let body = resp.body().as_ref().map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
            Ok((session, Handshake { status: resp.status().as_u16(), headers: header_pairs(resp.headers()), body }))
        }
        Ok(Err(e)) => Err(anyhow!(explain(&host, c.insecure, &e))),
    }
}

fn header_pairs(h: &http::HeaderMap) -> Vec<(String, String)> {
    h.iter().map(|(k, v)| (k.to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned())).collect()
}

fn explain(host: &str, insecure: bool, e: &WsError) -> String {
    match e {
        WsError::Io(io) => match io.kind() {
            std::io::ErrorKind::ConnectionRefused => format!("连不上 {host}：服务没有启动，或端口 / 地址不对（{io}）"),
            std::io::ErrorKind::TimedOut => format!("连接 {host} 超时：网络不通，或防火墙拦住了（{io}）"),
            _ if io.to_string().to_lowercase().contains("lookup") || io.to_string().contains("nodename") => {
                format!("找不到主机 {host}：域名写错了，或当前网络 / VPN 访问不到它（{io}）")
            }
            _ => format!("连接 {host} 失败（{io}）"),
        },
        WsError::Tls(t) => tls::explain_tls_error(host, insecure, &t.to_string()),
        WsError::Url(u) => format!("WebSocket 地址无效：{u}"),
        WsError::Protocol(p) => format!("{host} 的握手不是 WebSocket：路径不对，或服务没有在这个地址提供 WebSocket（{p}）"),
        other => format!("WebSocket 连接失败：{other}"),
    }
}

impl WsSession {
    pub fn is_open(&self) -> bool {
        self.ws.is_some() && self.closed_by_server.is_none()
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    fn push(&mut self, sent: bool, kind: &str, data: String) -> usize {
        let at_ms = self.elapsed_ms();
        self.frames.push(Frame { sent, kind: kind.into(), data, at_ms });
        self.frames.len() - 1
    }

    fn send(&mut self, msg: Message) -> Result<()> {
        if let Some((code, reason)) = &self.closed_by_server {
            bail!("连接已被服务端关闭（{code}{}），不能再发送", if reason.is_empty() { String::new() } else { format!(" {reason}") });
        }
        let ws = self.ws.as_mut().ok_or_else(|| anyhow!("连接已关闭"))?;
        self.rt.block_on(ws.send(msg)).map_err(|e| anyhow!("发送失败：{e}"))
    }

    pub fn send_text(&mut self, text: &str) -> Result<()> {
        self.send(Message::text(text.to_string()))?;
        self.push(true, "text", text.to_string());
        Ok(())
    }

    pub fn send_binary(&mut self, bytes: Vec<u8>) -> Result<()> {
        use base64::Engine;
        let shown = base64::engine::general_purpose::STANDARD.encode(&bytes);
        self.send(Message::binary(bytes))?;
        self.push(true, "binary", shown);
        Ok(())
    }

    /// 等下一条数据消息（ping / pong 自动处理，不计入）
    pub fn recv(&mut self, timeout: Duration) -> Result<Recv> {
        use base64::Engine;
        if self.closed_by_server.is_some() {
            return Ok(Recv::Closed);
        }
        let Some(ws) = self.ws.as_mut() else { return Ok(Recv::Closed) };
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let next = self.rt.block_on(async { tokio::time::timeout_at(deadline, ws.next()).await });
            let msg = match next {
                Err(_) => return Ok(Recv::Timeout),
                Ok(None) => {
                    self.closed_by_server = Some((1006, "连接断开".into()));
                    return Ok(Recv::Closed);
                }
                Ok(Some(Err(WsError::ConnectionClosed | WsError::AlreadyClosed))) => {
                    self.closed_by_server.get_or_insert((1006, "连接断开".into()));
                    return Ok(Recv::Closed);
                }
                Ok(Some(Err(e))) => {
                    self.closed_by_server = Some((1006, e.to_string()));
                    return Ok(Recv::Closed);
                }
                Ok(Some(Ok(m))) => m,
            };
            match msg {
                Message::Text(t) => {
                    self.received_bytes += t.len() as u64;
                    return Ok(Recv::Message(self.push(false, "text", t.to_string())));
                }
                Message::Binary(b) => {
                    self.received_bytes += b.len() as u64;
                    return Ok(Recv::Message(self.push(false, "binary", base64::engine::general_purpose::STANDARD.encode(&b))));
                }
                Message::Close(frame) => {
                    let (code, reason) = frame.map(|f| (u16::from(f.code), f.reason.to_string())).unwrap_or((1005, String::new()));
                    self.push(false, "close", format!("{code}{}", if reason.is_empty() { String::new() } else { format!(" {reason}") }));
                    self.closed_by_server = Some((code, reason));
                    return Ok(Recv::Closed);
                }
                _ => continue,
            }
        }
    }

    /// 收集 duration 内到达的全部消息（静候）
    pub fn drain_for(&mut self, duration: Duration) -> Result<()> {
        let until = Instant::now() + duration;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() || !self.is_open() {
                return Ok(());
            }
            if self.recv(left)? == Recv::Closed {
                return Ok(());
            }
        }
    }

    /// 主动关闭：发关闭帧，最多等 1 秒对方回应（期间到达的消息仍记录）
    pub fn close(&mut self, code: u16) -> Result<()> {
        if !self.is_open() {
            self.ws = None;
            return Ok(());
        }
        let ws = self.ws.as_mut().unwrap();
        let frame = CloseFrame { code: CloseCode::from(code), reason: "".into() };
        let _ = self.rt.block_on(ws.send(Message::Close(Some(frame))));
        self.push(true, "close", code.to_string());
        let deadline = Instant::now() + Duration::from_secs(1);
        while self.closed_by_server.is_none() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || matches!(self.recv(left), Ok(Recv::Timeout) | Err(_)) {
                break;
            }
        }
        // 对方回应的关闭帧不算「服务端关闭」
        if self.closed_by_server.as_ref().is_some_and(|(c, _)| *c == code) {
            if let Some(last) = self.frames.last() {
                if !last.sent && last.kind == "close" {
                    self.frames.pop();
                }
            }
            self.closed_by_server = None;
        }
        self.ws = None;
        Ok(())
    }
}

/// 收到的消息 → 断言 / 捕获看到的值：JSON 文本解析成 JSON，其余是字符串（二进制为 base64）
pub fn frame_value(f: &Frame) -> serde_json::Value {
    if f.kind == "text" {
        serde_json::from_str(&f.data).unwrap_or_else(|_| serde_json::Value::String(f.data.clone()))
    } else {
        serde_json::Value::String(f.data.clone())
    }
}

/// 等价命令（websocat；每行一条要发送的消息）
pub fn websocat_command(c: &WsConnect, sends: &[String]) -> String {
    use super::shell_quote;
    let mut argv = vec!["websocat".to_string()];
    if c.insecure {
        argv.push("-k".into());
    }
    for (k, v) in &c.headers {
        argv.push("-H".into());
        argv.push(format!("{k}: {v}"));
    }
    if !c.subprotocols.is_empty() {
        argv.push("--protocol".into());
        argv.push(c.subprotocols.join(","));
    }
    argv.push(normalize_url(&c.url).unwrap_or_else(|_| c.url.clone()));
    let cmd = argv.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ");
    if sends.is_empty() {
        cmd
    } else {
        format!("printf '%s\\n' {} | {cmd}", sends.iter().map(|s| shell_quote(s)).collect::<Vec<_>>().join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_normalize_to_ws_schemes() {
        assert_eq!(normalize_url("ws://h:1/a").unwrap(), "ws://h:1/a");
        assert_eq!(normalize_url("https://h/a?x=1").unwrap(), "wss://h/a?x=1");
        assert!(normalize_url("h/a").unwrap_err().to_string().contains("缺少协议"));
        assert!(normalize_url("ftp://h").is_err());
        assert_eq!(host_of("wss://u:p@h.example:8443/x?y"), "h.example:8443");
    }

    #[test]
    fn received_frames_become_json_when_possible() {
        let f = |kind: &str, data: &str| Frame { sent: false, kind: kind.into(), data: data.into(), at_ms: 0 };
        assert_eq!(frame_value(&f("text", r#"{"type":"joined"}"#)), serde_json::json!({"type": "joined"}));
        assert_eq!(frame_value(&f("text", "pong")), serde_json::json!("pong"));
        assert_eq!(frame_value(&f("binary", "AAE=")), serde_json::json!("AAE="));
    }

    #[test]
    fn websocat_command_reproduces_the_conversation() {
        let c = WsConnect { url: "wss://h/chat".into(), headers: vec![("Authorization".into(), "Bearer t".into())], subprotocols: vec!["chat.v1".into()], timeout_ms: 1000, insecure: true, ..Default::default() };
        assert_eq!(
            websocat_command(&c, &[r#"{"type":"join"}"#.into(), "ping".into()]),
            r#"printf '%s\n' '{"type":"join"}' ping | websocat -k -H 'Authorization: Bearer t' --protocol chat.v1 wss://h/chat"#
        );
    }

    #[test]
    fn refused_connection_is_explained() {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let e = connect(&WsConnect { url: format!("ws://127.0.0.1:{port}/x"), timeout_ms: 2000, ..Default::default() }).err().unwrap().to_string();
        assert!(e.starts_with(&format!("连不上 127.0.0.1:{port}")), "{e}");
    }
}
