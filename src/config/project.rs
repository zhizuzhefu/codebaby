use super::StringVars;
use serde::{Deserialize, Serialize};

/// 项目根配置 apicase.json
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Project {
    /// 配置格式版本（当前 1）
    #[serde(default = "default_version")]
    pub version: u32,
    /// 默认环境名（apicase run 未指定 -e 时使用）
    #[serde(default = "default_env")]
    pub default_env: String,
    /// 全局变量（最低优先级）
    #[serde(default)]
    pub vars: StringVars,
    /// 默认请求超时（毫秒）
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
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
        }
    }
}
