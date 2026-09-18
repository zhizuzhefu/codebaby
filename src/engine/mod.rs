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

/// 传输层抽象：当前实现为 curl 子进程，未来可替换 libcurl FFI / 内置 HTTP
pub trait Transport {
    /// 编译为可执行表示（不执行），dry-run 与实际执行共用
    fn compile(&self, req: &ResolvedRequest) -> Result<CurlInvocation>;

    /// 执行
    fn run(&self, inv: &CurlInvocation) -> Result<Response>;
}

/// 一次 curl 调用的完整描述（argv + body 落盘文件）
#[derive(Debug)]
pub struct CurlInvocation {
    pub argv: Vec<String>,
    /// body 临时文件句柄：保持存活到 curl 执行完毕（Drop 时自动清理）
    #[allow(dead_code)]
    pub body: Option<tempfile::TempPath>,
}
