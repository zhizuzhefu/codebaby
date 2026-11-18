pub mod case;
pub mod collection;
pub mod project;
pub mod protocols;
pub mod ui;
pub mod vars_map;

pub use case::{enabled_kv, BodySpec, CasesSpec, DataRow, KvValue, MultipartValue, TestCase};
pub use collection::{env_path, list_envs, load_env, local_env_path, Collection, EnvFile, LOCAL_SUFFIX};
pub use project::{GrpcConfig, Project, ProxyConfig, Workflow, WorkflowStep};
pub use protocols::{GrpcSpec, Protocol, WsAction, WsClose, WsSpec, WsStep};
pub use ui::{AndroidSpec, CaptureSource, Locator, UiAction, UiState, UiStep, WebSpec};
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
