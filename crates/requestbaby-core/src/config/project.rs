use super::StringVars;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 项目根配置 requestbaby.json
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Project {
    /// 配置格式版本（当前 1）
    #[serde(default = "default_version")]
    pub version: u32,
    /// 默认环境名（未指定 -e 时使用）
    #[serde(default = "default_env")]
    pub default_env: String,
    /// 全局变量（最低优先级）
    #[serde(default)]
    pub vars: StringVars,
    /// 默认请求超时（毫秒）
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    /// 命名工作流：多阶段编排（数据参数经 runtime vars 跨阶段自动传递）
    #[serde(default)]
    pub workflows: BTreeMap<String, Workflow>,
}

/// 一个工作流 = 有序步骤列表
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Workflow {
    /// 工作流显示名（缺省用 key）
    #[serde(default)]
    pub name: String,
    /// 有序步骤
    pub steps: Vec<WorkflowStep>,
}

/// 单个步骤 = 一次 run（路径 + 过滤），变量与前序步骤自动共享
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkflowStep {
    /// 步骤名（缺省用 run 路径）
    #[serde(default)]
    pub name: String,
    /// 运行目标（文件/目录）
    pub run: String,
    /// 标签过滤
    #[serde(default)]
    pub tags: Vec<String>,
    /// 用例名过滤
    #[serde(default)]
    pub name_filter: String,
}

fn default_version() -> u32 {
    1
}
fn default_env() -> String {
    "dev".to_string()
}
fn default_timeout() -> u64 {
    30_000
}

impl Default for Project {
    fn default() -> Self {
        Self {
            version: default_version(),
            default_env: default_env(),
            vars: StringVars::default(),
            timeout_ms: default_timeout(),
            workflows: BTreeMap::new(),
        }
    }
}
