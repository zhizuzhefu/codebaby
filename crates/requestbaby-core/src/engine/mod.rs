pub mod android;
pub mod cdp;
pub mod curl;
pub mod desktop;
pub mod grpc;
pub mod proxy;
pub mod tls;
pub mod ui;
pub mod web;
pub mod ws;

pub use curl::CurlTransport;

use anyhow::Result;

/// 变量替换与 pre 脚本处理后的最终请求
#[derive(Debug, Clone, Default)]
pub struct ResolvedRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    /// multipart/form-data 字段（非空时取代 body）
    pub multipart: Vec<FormPart>,
    pub timeout_ms: u64,
    pub retry: u32,
    pub insecure: bool,
    pub follow_redirects: bool,
    /// 代理规则（缺省跟随系统：环境变量 → 系统代理设置 → 直连）
    pub proxy: proxy::ProxyPolicy,
}

/// multipart/form-data 的一个字段
#[derive(Debug, Clone)]
pub struct FormPart {
    pub name: String,
    pub value: PartValue,
}

#[derive(Debug, Clone)]
pub enum PartValue {
    Text(String),
    /// 上传文件（绝对路径）；filename / content_type 缺省由 libcurl 推断
    File { path: std::path::PathBuf, filename: Option<String>, content_type: Option<String> },
}

/// 响应（传输错误时 status=0 且 transport_error 有值）
#[derive(Debug, Clone, Default)]
pub struct Response {
    pub status: u16,
    /// 响应头（保留重复键，如 Set-Cookie）
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub time_ms: u64,
    pub size: u64,
    pub transport_error: Option<String>,
}

/// 一条消息（WebSocket 收发 / gRPC 流式消息），对话记录与界面时间线用
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    /// true = 本端发出，false = 收到
    pub sent: bool,
    /// text / binary / close / message（gRPC 消息，data 为 JSON）
    pub kind: String,
    /// 文本内容；二进制为 base64；关闭帧为「关闭码 原因」
    pub data: String,
    /// 距连接开始的毫秒数
    pub at_ms: u64,
}

impl Response {
    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// 传输引擎：CurlTransport（libcurl binding，静态链接，零外部进程依赖）
pub trait Transport: Send + Sync {
    fn execute(&self, req: &ResolvedRequest) -> Result<Response>;
}

/// curl 命令展示描述（dry-run / -v / 「复制为 cURL」，不执行）
#[derive(Debug)]
pub struct CurlInvocation {
    /// 完整参数（含请求体 `--data-raw` / `-F`）
    pub argv: Vec<String>,
    pub body: Option<String>,
}

impl CurlInvocation {
    /// 可直接粘贴到终端执行的命令（按 POSIX shell 规则加引号）
    pub fn shell(&self) -> String {
        self.argv.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ")
    }
}

/// POSIX shell 引号：安全字符原样输出，其余用单引号包裹（内部 ' 转为 '\''）
pub fn shell_quote(s: &str) -> String {
    let safe = !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c));
    if safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// 内嵌 libcurl 版本（doctor / Desktop 关于页）
pub fn libcurl_version() -> String {
    ::curl::Version::get().version().to_string()
}
