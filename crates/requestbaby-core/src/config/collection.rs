use super::StringVars;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 环境文件 envs/<name>.json
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EnvFile {
    #[serde(default)]
    pub vars: StringVars,
}

/// 本机覆盖文件后缀：`envs/<env>.local.json` 只放本机的密钥 / 端口等，不入库（scaffold 与写入时确保 .gitignore）
pub const LOCAL_SUFFIX: &str = ".local.json";

pub fn env_path(root: &Path, env: &str) -> PathBuf {
    root.join("envs").join(format!("{env}.json"))
}

pub fn local_env_path(root: &Path, env: &str) -> PathBuf {
    root.join("envs").join(format!("{env}{LOCAL_SUFFIX}"))
}

/// 项目的环境名（排序；不含 `.local.json` 覆盖文件）
pub fn list_envs(root: &Path) -> Vec<String> {
    let mut envs: Vec<String> = std::fs::read_dir(root.join("envs"))
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if name.ends_with(LOCAL_SUFFIX) {
                        return None;
                    }
                    name.strip_suffix(".json").map(String::from)
                })
                .collect()
        })
        .unwrap_or_default();
    envs.sort();
    envs
}

/// 加载环境：`envs/<env>.json` 叠加 `envs/<env>.local.json`（vars 同名覆盖）。
/// 两者都不存在 → None
pub fn load_env(root: &Path, env: &str) -> anyhow::Result<Option<EnvFile>> {
    use anyhow::Context;
    let base_path = env_path(root, env);
    let local_path = local_env_path(root, env);
    let mut base = match base_path.exists() {
        true => Some(super::load_strict::<EnvFile>(&base_path).with_context(|| format!("环境文件 {env} 加载失败"))?),
        false => None,
    };
    if local_path.exists() {
        let local = super::load_strict::<EnvFile>(&local_path).with_context(|| format!("本机覆盖 envs/{env}{LOCAL_SUFFIX} 加载失败"))?;
        base.get_or_insert_with(|| EnvFile { vars: StringVars::default() }).vars.0.extend(local.vars.0);
    }
    Ok(base)
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
    pub headers: BTreeMap<String, super::KvValue>,
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
