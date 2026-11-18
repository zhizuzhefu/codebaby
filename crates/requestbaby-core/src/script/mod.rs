pub mod quickjs;

pub use quickjs::QuickJsHost;

use crate::vars::VarVal;
use anyhow::Result;
use std::collections::BTreeMap;

/// pre 脚本可操作的请求视图
#[derive(Debug, Clone)]
pub struct ReqData {
    pub method: String,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: Option<String>,
}

/// post/tests 脚本可见的响应视图
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RespData {
    /// HTTP / WebSocket：HTTP 状态码（握手成功 101）；gRPC：gRPC 状态码（0 = OK）
    pub status: u16,
    pub time_ms: u64,
    pub size: u64,
    /// HTTP 响应头 / gRPC 响应元数据 / WebSocket 握手响应头
    pub headers: Vec<(String, String)>,
    /// HTTP 响应体 / gRPC 响应消息（JSON；流式为数组）/ WebSocket 收到的消息数组（JSON）
    pub body: String,
    #[serde(default, skip_serializing_if = "crate::config::Protocol::is_http")]
    pub protocol: crate::config::Protocol,
    /// 消息时间线：WebSocket 收发 / gRPC 请求与响应消息
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frames: Vec<crate::engine::Frame>,
    /// 界面用例（Web / Android）的证据：每一步的记录与截图
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<crate::engine::ui::UiEvidence>,
}

impl RespData {
    /// HTTP 响应（无消息时间线）
    pub fn http(status: u16, time_ms: u64, size: u64, headers: Vec<(String, String)>, body: String) -> Self {
        Self { status, time_ms, size, headers, body, protocol: crate::config::Protocol::Http, frames: vec![], evidence: None }
    }
}

/// 单条断言结果
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TestOutcome {
    pub expr: String,
    pub pass: bool,
    pub detail: Option<String>,
}

/// 脚本执行统一产出：回写数据 + 日志 + 错误
#[derive(Debug, Clone, Default)]
pub struct ScriptOutcome {
    pub req: Option<ReqData>,
    /// 脚本写入的变量（保留原生类型：Num/Bool 注入还原）
    pub vars: BTreeMap<String, VarVal>,
    pub logs: Vec<String>,
    /// 脚本运行时异常（语法错误/抛出）
    pub error: Option<String>,
}

/// 脚本宿主抽象：QuickJS 为当前实现
pub trait ScriptHost {
    /// 前置脚本：注入 req/vars/env，返回改写后的请求与变量
    fn run_pre(&self, req: ReqData, vars: &BTreeMap<String, VarVal>, env: &BTreeMap<String, VarVal>, script: &str) -> Result<ScriptOutcome>;

    /// 后置脚本：注入 res/vars/env，返回变量回写
    fn run_post(&self, resp: &RespData, vars: &BTreeMap<String, VarVal>, env: &BTreeMap<String, VarVal>, script: &str) -> Result<ScriptOutcome>;

    /// 集合级脚本（setup/teardown）：注入 vars/env/http（可发请求造数/清数）
    fn run_scoped(&self, vars: &BTreeMap<String, VarVal>, env: &BTreeMap<String, VarVal>, script: &str) -> Result<ScriptOutcome>;

    /// 断言求值：注入 res/vars/env，逐条 eval
    fn eval_tests(&self, resp: &RespData, vars: &BTreeMap<String, VarVal>, env: &BTreeMap<String, VarVal>, tests: &[String]) -> Result<Vec<TestOutcome>>;

    /// 语法检查（validate 用）：new Function 编译不执行
    fn compile_check(&self, script: &str) -> Result<()>;
}
