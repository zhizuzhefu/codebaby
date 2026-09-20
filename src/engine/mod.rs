pub mod curl;

pub use curl::CurlTransport;

use anyhow::Result;

/// 变量替换与 pre 脚本处理后的最终请求
#[derive(Debug, Clone)]
pub struct ResolvedRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    pub timeout_ms: u64,
    pub retry: u32,
    pub insecure: bool,
    pub follow_redirects: bool,
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

impl Response {
    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// 传输引擎：CurlTransport（libcurl binding，静态链接，零外部进程依赖）
pub trait Transport: Send + Sync {
    fn execute(&self, req: &ResolvedRequest) -> Result<Response>;
}

/// curl 命令展示描述（argv + body；dry-run / -v 展示用，不执行）
#[derive(Debug)]
pub struct CurlInvocation {
    pub argv: Vec<String>,
    pub body: Option<String>,
}
