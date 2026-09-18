use super::StringVars;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 环境文件 envs/<name>.json
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EnvFile {
    #[serde(default)]
    pub vars: StringVars,
}

/// 集合配置 collection.json（任意集合层级可放）
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Collection {
    /// 集合显示名（缺省用目录名）
    #[serde(default)]
    pub name: String,
    /// 集合级变量（覆盖环境变量，被运行时 vars 覆盖）
    #[serde(default)]
    pub vars: StringVars,
    /// 集合级默认 headers（逐级继承，用例级同名覆盖；值支持 {{var}}）
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// 集合级标签（附加到该集合下所有用例）
    #[serde(default)]
    pub tags: Vec<String>,
    /// 进入集合（首个用例前）执行：可写 vars、可 http.request 造数
    #[serde(default)]
    pub setup: String,
    /// 离开集合（最后用例后，无论成败）执行：清理数据
    #[serde(default)]
    pub teardown: String,
}
