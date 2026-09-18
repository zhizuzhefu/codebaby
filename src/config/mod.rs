pub mod case;
pub mod collection;
pub mod project;
pub mod vars_map;

pub use case::TestCase;
pub use collection::{Collection, EnvFile};
pub use project::Project;
pub use vars_map::StringVars;

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use std::path::Path;

/// 严格 JSON 解析：未知字段报错（第一层校验）
pub fn load_strict<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("读取失败: {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("解析失败: {}", path.display()))
}
