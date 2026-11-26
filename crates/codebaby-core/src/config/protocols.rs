//! 协议：一个用例的「请求」部分六选一 —— `request`（HTTP）/ `grpc` / `ws`（WebSocket）/
//! `web`（浏览器）/ `android`（手机应用）/ `desktop`（Mac 桌面应用），后三者见 [`super::ui`]。
//! 其余部分（expect / capture / 变量 / 集合 / 流程 / 报告）与 HTTP 完全一致：
//! - gRPC：`expect.status` 是 gRPC 状态（`"OK"` / `"NOT_FOUND"` 或数字），`headers` 是响应元数据，
//!   `body` 是响应消息（服务端流 / 双向流时是消息数组）
//! - WebSocket：`expect.status` 是握手状态（成功 101），`body` 是收到的消息数组；
//!   对话过程写在 `ws.steps`（发送 / 接收并断言 / 等到某条消息 / 静候 / 关闭）
use super::case::KvValue;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    #[default]
    Http,
    Grpc,
    Ws,
    Web,
    Android,
    Desktop,
}

impl Protocol {
    pub fn is_http(&self) -> bool {
        *self == Protocol::Http
    }

    /// 界面与报告里的名字
    pub fn label(&self) -> &'static str {
        match self {
            Protocol::Http => "HTTP / HTTPS",
            Protocol::Grpc => "gRPC",
            Protocol::Ws => "WebSocket",
            Protocol::Web => "Web",
            Protocol::Android => "Android",
            Protocol::Desktop => "Mac 桌面",
        }
    }

    /// 用例 JSON 里的请求键
    pub fn key(&self) -> &'static str {
        match self {
            Protocol::Http => "request",
            Protocol::Grpc => "grpc",
            Protocol::Ws => "ws",
            Protocol::Web => "web",
            Protocol::Android => "android",
            Protocol::Desktop => "desktop",
        }
    }

    /// 界面自动化（逐步操作 + 截图留证）
    pub fn is_ui(&self) -> bool {
        self.ui_kind().is_some()
    }

    /// 界面自动化的驱动种类（非界面协议为 None）
    pub fn ui_kind(&self) -> Option<super::ui::UiKind> {
        use super::ui::UiKind;
        match self {
            Protocol::Web => Some(UiKind::Web),
            Protocol::Android => Some(UiKind::Android),
            Protocol::Desktop => Some(UiKind::Desktop),
            _ => None,
        }
    }

    pub const ALL: [Protocol; 6] = [Protocol::Http, Protocol::Grpc, Protocol::Ws, Protocol::Web, Protocol::Android, Protocol::Desktop];
}

/// gRPC 调用
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct GrpcSpec {
    /// 服务地址：`host:port` / `grpc://host:port`（明文），`grpcs://host:port` / `https://host`（TLS）；支持 {{var}}
    pub target: String,
    /// 完整方法名：`包.服务/方法`，如 `user.v1.UserService/GetUser`
    pub method: String,
    /// 请求消息（protobuf 的 JSON 写法；字段名用 .proto 里的名字或 lowerCamelCase 均可）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<serde_json::Value>,
    /// 客户端流 / 双向流：按顺序发送的多条消息（与 message 二选一）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<Vec<serde_json::Value>>,
    /// 请求元数据（相当于 HTTP 请求头）；值同样支持 `{"value": "...", "disabled": true}`
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, KvValue>,
    /// 超时毫秒（缺省用项目配置）；同时作为 grpc-timeout 发给服务端
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// TLS 时跳过证书校验（自签证书的测试环境）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub insecure: Option<bool>,
}

impl GrpcSpec {
    /// 待发送的消息（未写时发一条空消息）
    pub fn outgoing(&self) -> Vec<serde_json::Value> {
        match (&self.messages, &self.message) {
            (Some(ms), _) => ms.clone(),
            (None, Some(m)) => vec![m.clone()],
            (None, None) => vec![serde_json::json!({})],
        }
    }

    pub fn texts(&self) -> Vec<String> {
        let mut t = vec![self.target.clone(), self.method.clone()];
        t.extend(self.metadata.iter().flat_map(|(k, v)| [k.clone(), v.value().to_string()]));
        t.extend(self.outgoing().iter().map(|m| m.to_string()));
        t
    }
}

/// WebSocket 对话
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WsSpec {
    /// 连接地址：`ws://` / `wss://`（也接受 http(s)://，按同一主机连接）；支持 {{var}}
    pub url: String,
    /// 握手请求头
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, KvValue>,
    /// 查询参数（追加到 url）
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub query: BTreeMap<String, KvValue>,
    /// 子协议（Sec-WebSocket-Protocol）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subprotocols: Vec<String>,
    /// 对话步骤，按顺序执行；全部完成后正常关闭连接
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<WsStep>,
    /// 连接与每次等待的默认超时毫秒（缺省用项目配置）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// wss 时跳过证书校验
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub insecure: Option<bool>,
}

impl WsSpec {
    pub fn texts(&self) -> Vec<String> {
        let mut t = vec![self.url.clone()];
        t.extend(self.headers.values().map(|v| v.value().to_string()));
        t.extend(self.query.iter().flat_map(|(k, v)| [k.clone(), v.value().to_string()]));
        for s in &self.steps {
            if let Some(v) = &s.send {
                t.push(v.to_string());
            }
            if let Some(v) = &s.send_binary {
                t.push(v.clone());
            }
            for m in [&s.receive, &s.wait_for].into_iter().flatten() {
                t.extend(m.values().map(|v| v.to_string()));
            }
        }
        t
    }

    /// 步骤里 capture 的变量名
    pub fn captures(&self) -> Vec<String> {
        self.steps.iter().flat_map(|s| s.capture.keys().cloned()).collect()
    }
}

/// 一个对话步骤：send / sendBinary / receive / waitFor / wait / close 恰好写一个
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WsStep {
    /// 发送文本消息：字符串原样发送，对象 / 数组 / 数字按 JSON 发送；支持 {{var}}（含前面步骤捕获的变量）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send: Option<serde_json::Value>,
    /// 发送二进制消息（base64）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_binary: Option<String>,
    /// 接收下一条消息并断言：路径 → 期望（与 expect.body 写法相同，`$` = 整条消息）；`{}` = 只要收到
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receive: Option<serde_json::Map<String, serde_json::Value>>,
    /// 一直等到满足条件的消息（中间的心跳等其他消息跳过，仍记录在对话里）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_for: Option<serde_json::Map<String, serde_json::Value>>,
    /// 静候毫秒数，期间收到的消息照常记录
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<u64>,
    /// 主动关闭连接：`true`（1000）或关闭码
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close: Option<WsClose>,
    /// receive / waitFor 收到的消息里取值：变量名 → 路径（后续步骤与用例可用）
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub capture: serde_json::Map<String, serde_json::Value>,
    /// 本步等待超时毫秒（缺省用 ws.timeoutMs）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum WsClose {
    Flag(bool),
    Code(u16),
}

impl WsClose {
    pub fn code(&self) -> u16 {
        match self {
            WsClose::Flag(_) => 1000,
            WsClose::Code(c) => *c,
        }
    }
}

/// 步骤动作（界面与报告用）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WsAction {
    Send,
    SendBinary,
    Receive,
    WaitFor,
    Wait,
    Close,
}

impl WsAction {
    pub const ALL: [WsAction; 6] = [WsAction::Send, WsAction::SendBinary, WsAction::Receive, WsAction::WaitFor, WsAction::Wait, WsAction::Close];

    pub fn key(&self) -> &'static str {
        match self {
            WsAction::Send => "send",
            WsAction::SendBinary => "sendBinary",
            WsAction::Receive => "receive",
            WsAction::WaitFor => "waitFor",
            WsAction::Wait => "wait",
            WsAction::Close => "close",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            WsAction::Send => "发送",
            WsAction::SendBinary => "发送二进制",
            WsAction::Receive => "接收",
            WsAction::WaitFor => "等到",
            WsAction::Wait => "静候",
            WsAction::Close => "关闭",
        }
    }
}

impl WsStep {
    /// 写了哪些动作（合法步骤恰好一个）
    pub fn actions(&self) -> Vec<WsAction> {
        let mut out = vec![];
        if self.send.is_some() {
            out.push(WsAction::Send);
        }
        if self.send_binary.is_some() {
            out.push(WsAction::SendBinary);
        }
        if self.receive.is_some() {
            out.push(WsAction::Receive);
        }
        if self.wait_for.is_some() {
            out.push(WsAction::WaitFor);
        }
        if self.wait.is_some() {
            out.push(WsAction::Wait);
        }
        if self.close.is_some() {
            out.push(WsAction::Close);
        }
        out
    }

    pub fn action(&self) -> Option<WsAction> {
        match self.actions().as_slice() {
            [one] => Some(*one),
            _ => None,
        }
    }
}

/// gRPC 状态码名（与 grpc-go / grpcurl 一致）
pub const GRPC_CODES: [&str; 17] = [
    "OK",
    "CANCELLED",
    "UNKNOWN",
    "INVALID_ARGUMENT",
    "DEADLINE_EXCEEDED",
    "NOT_FOUND",
    "ALREADY_EXISTS",
    "PERMISSION_DENIED",
    "RESOURCE_EXHAUSTED",
    "FAILED_PRECONDITION",
    "ABORTED",
    "OUT_OF_RANGE",
    "UNIMPLEMENTED",
    "INTERNAL",
    "UNAVAILABLE",
    "DATA_LOSS",
    "UNAUTHENTICATED",
];

pub fn grpc_code_name(code: u16) -> String {
    GRPC_CODES.get(code as usize).map(|s| s.to_string()).unwrap_or_else(|| format!("CODE_{code}"))
}

/// "NOT_FOUND" / "not_found" / "5" → 5
pub fn parse_grpc_code(text: &str) -> Option<u16> {
    let t = text.trim();
    if let Ok(n) = t.parse::<u16>() {
        return (n < 17).then_some(n);
    }
    GRPC_CODES.iter().position(|c| c.eq_ignore_ascii_case(t)).map(|i| i as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grpc_codes_parse_by_name_or_number() {
        assert_eq!(parse_grpc_code("OK"), Some(0));
        assert_eq!(parse_grpc_code("not_found"), Some(5));
        assert_eq!(parse_grpc_code("16"), Some(16));
        assert_eq!(parse_grpc_code("17"), None);
        assert_eq!(parse_grpc_code("NOPE"), None);
        assert_eq!(grpc_code_name(7), "PERMISSION_DENIED");
    }

    #[test]
    fn ws_step_has_exactly_one_action() {
        let s: WsStep = serde_json::from_str(r#"{"send":{"type":"join"}}"#).unwrap();
        assert_eq!(s.action(), Some(WsAction::Send));
        let s: WsStep = serde_json::from_str(r#"{"receive":{"type":"joined"},"capture":{"sid":"sid"}}"#).unwrap();
        assert_eq!(s.action(), Some(WsAction::Receive));
        let s: WsStep = serde_json::from_str(r#"{"send":"a","wait":10}"#).unwrap();
        assert_eq!(s.action(), None);
        let s: WsStep = serde_json::from_str(r#"{"close":4001}"#).unwrap();
        assert_eq!(s.close.unwrap().code(), 4001);
    }
}
