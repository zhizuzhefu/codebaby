pub mod quickjs;

pub use quickjs::QuickJsHost;

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
#[derive(Debug, Clone)]
pub struct RespData {
    pub status: u16,
    pub time_ms: u64,
    pub size: u64,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// 单条断言结果
#[derive(Debug, Clone)]
pub struct TestOutcome {
    pub expr: String,
    pub pass: bool,
    pub detail: Option<String>,
}

/// 脚本执行统一产出：回写数据 + 日志 + 错误
#[derive(Debug, Clone, Default)]
pub struct ScriptOutcome {
    pub req: Option<ReqData>,
    pub vars: BTreeMap<String, String>,
    pub logs: Vec<String>,
    /// 脚本运行时异常（语法错误/抛出）
    pub error: Option<String>,
}

/// 脚本宿主抽象：QuickJS 为当前实现
pub trait ScriptHost {
    /// 前置脚本：注入 req/vars/env，返回改写后的请求与变量
    fn run_pre(&self, req: ReqData, vars: &BTreeMap<String, String>, env: &BTreeMap<String, String>, script: &str) -> Result<ScriptOutcome>;

    /// 后置脚本：注入 res/vars/env，返回变量回写
    fn run_post(&self, resp: &RespData, vars: &BTreeMap<String, String>, env: &BTreeMap<String, String>, script: &str) -> Result<ScriptOutcome>;

    /// 集合级脚本（setup/teardown）：注入 vars/env/http（可发请求造数/清数）
    fn run_scoped(&self, vars: &BTreeMap<String, String>, env: &BTreeMap<String, String>, script: &str) -> Result<ScriptOutcome>;

    /// 断言求值：注入 res/vars/env，逐条 eval
    fn eval_tests(&self, resp: &RespData, vars: &BTreeMap<String, String>, env: &BTreeMap<String, String>, tests: &[String]) -> Result<Vec<TestOutcome>>;

    /// 语法检查（validate 用）：new Function 编译不执行
    fn compile_check(&self, script: &str) -> Result<()>;
}
