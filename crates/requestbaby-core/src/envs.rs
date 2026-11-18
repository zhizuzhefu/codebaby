//! 环境管理：envs/<name>.json 的增删改查与默认环境。CLI `requestbaby env …` 与 Desktop 环境管理页同一实现。
//!
//! - 重命名同步：本机覆盖 envs/<name>.local.json、requestbaby.json 的 defaultEnv、固定在该环境的流程（workflows.*.env）
//! - 删除：默认环境、被流程固定的环境不能删（先改掉引用）；环境文件与本机覆盖一起移到废纸篓
//! - 复制：只复制入库的环境文件，本机覆盖（密钥）不复制
use crate::config::{self, load_strict, EnvFile, Project};
use crate::runner::discover;
use crate::scaffold::{self, Deleted};
use crate::workflow;
use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// 环境概览（环境管理页 / `requestbaby env list`）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvInfo {
    pub name: String,
    /// 环境文件中的变量数
    pub vars: usize,
    /// 本机覆盖文件存在时其中的变量数
    pub local_vars: Option<usize>,
    pub is_default: bool,
    /// 固定在此环境的流程 key
    pub pinned_by: Vec<String>,
    pub base_url: Option<String>,
    /// 环境文件无法解析时的原因
    pub error: Option<String>,
}

pub fn list(root: &Path) -> Result<Vec<EnvInfo>> {
    let project = load_project(root)?;
    Ok(config::list_envs(root)
        .into_iter()
        .map(|name| {
            let (vars, base_url, error) = match load_strict::<EnvFile>(&config::env_path(root, &name)) {
                Ok(f) => (f.vars.len(), f.vars.get("baseUrl").map(|v| v.value.clone()), None),
                Err(e) => (0, None, Some(format!("{e:#}"))),
            };
            let local = config::local_env_path(root, &name);
            let local_vars = local.exists().then(|| load_strict::<EnvFile>(&local).map(|f| f.vars.len()).unwrap_or(0));
            EnvInfo {
                is_default: project.default_env == name,
                pinned_by: pinned_by(&project, &name),
                name,
                vars,
                local_vars,
                base_url,
                error,
            }
        })
        .collect())
}

/// 新建环境；from 给定时复制其变量（不含本机覆盖）
pub fn create(root: &Path, name: &str, from: Option<&str>) -> Result<PathBuf> {
    check_env_name(name)?;
    if let Some(src) = from {
        ensure_exists(root, src)?;
    }
    scaffold::new_env(root, name.trim(), from)
}

pub fn duplicate(root: &Path, src: &str, new_name: &str) -> Result<PathBuf> {
    create(root, new_name, Some(src))
}

/// 重命名环境：环境文件 + 本机覆盖 + defaultEnv + 固定在它上面的流程一起改
pub fn rename(root: &Path, old: &str, new_name: &str) -> Result<()> {
    ensure_exists(root, old)?;
    check_env_name(new_name)?;
    let new_name = new_name.trim();
    if new_name == old {
        return Ok(());
    }
    if config::env_path(root, new_name).exists() || config::local_env_path(root, new_name).exists() {
        bail!("环境已存在: {new_name}");
    }
    std::fs::rename(config::env_path(root, old), config::env_path(root, new_name)).with_context(|| format!("重命名环境 {old} 失败"))?;
    let local = config::local_env_path(root, old);
    if local.exists() {
        std::fs::rename(&local, config::local_env_path(root, new_name)).with_context(|| format!("重命名本机覆盖 {} 失败", local.display()))?;
    }
    workflow::edit_project(root, |doc| {
        if doc.get("defaultEnv").and_then(|v| v.as_str()) == Some(old) {
            doc.insert("defaultEnv".into(), new_name.into());
        }
        if let Some(wfs) = doc.get_mut("workflows").and_then(|w| w.as_object_mut()) {
            for wf in wfs.values_mut().filter_map(|w| w.as_object_mut()) {
                if wf.get("env").and_then(|v| v.as_str()) == Some(old) {
                    wf.insert("env".into(), new_name.into());
                }
            }
        }
        Ok(())
    })
}

/// 删除环境（环境文件 + 本机覆盖移到废纸篓）。默认环境、被流程固定的环境拒绝删除
pub fn delete(root: &Path, name: &str) -> Result<Deleted> {
    ensure_exists(root, name)?;
    let project = load_project(root)?;
    if project.default_env == name {
        bail!("{name} 是默认环境，不能删除：先把其他环境设为默认（requestbaby env default <环境>）");
    }
    let pinned = pinned_by(&project, name);
    if !pinned.is_empty() {
        bail!("流程 {} 固定在环境 {name}，不能删除：先修改这些流程的运行环境", pinned.join("、"));
    }
    let how = scaffold::delete(&config::env_path(root, name))?;
    let local = config::local_env_path(root, name);
    if local.exists() {
        scaffold::delete(&local)?;
    }
    Ok(how)
}

/// 设为默认环境（requestbaby.json 的 defaultEnv：不带 -e 的运行都用它）
pub fn set_default(root: &Path, name: &str) -> Result<()> {
    ensure_exists(root, name)?;
    workflow::edit_project(root, |doc| {
        doc.insert("defaultEnv".into(), name.into());
        Ok(())
    })
}

fn check_env_name(name: &str) -> Result<()> {
    scaffold::check_name(name)?;
    if name.trim().ends_with(".local") || name.trim().ends_with(".json") {
        bail!("环境名不能以 .local / .json 结尾（envs/<名称>.local.json 是本机覆盖文件）");
    }
    Ok(())
}

fn ensure_exists(root: &Path, name: &str) -> Result<()> {
    if config::env_path(root, name).exists() {
        return Ok(());
    }
    let known = config::list_envs(root);
    Err(anyhow!("环境不存在: {name}（现有：{}）", if known.is_empty() { "无".into() } else { known.join("、") }))
}

fn load_project(root: &Path) -> Result<Project> {
    let pf = discover::project_file(root).ok_or_else(|| anyhow!("缺少 requestbaby.json（{}）", root.display()))?;
    load_strict::<Project>(&pf).context("项目配置错误")
}

fn pinned_by(project: &Project, env: &str) -> Vec<String> {
    project.workflows.iter().filter(|(_, w)| w.env.as_deref() == Some(env)).map(|(k, _)| k.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        scaffold::init_project(dir.path()).unwrap();
        dir
    }

    fn doc(root: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(root.join("requestbaby.json")).unwrap()).unwrap()
    }

    #[test]
    fn crud_keeps_references_in_sync() {
        let dir = project();
        let root = dir.path();
        // 复制：变量跟着走，本机覆盖不复制
        std::fs::write(config::local_env_path(root, "dev"), r#"{"vars":{"token":"t"}}"#).unwrap();
        duplicate(root, "dev", "staging").unwrap();
        let staging = list(root).unwrap().into_iter().find(|e| e.name == "staging").unwrap();
        assert_eq!((staging.vars, staging.local_vars), (1, None));
        assert!(duplicate(root, "dev", "staging").unwrap_err().to_string().contains("已存在"));
        assert!(create(root, "x.local", None).is_err());
        assert!(create(root, "y", Some("nope")).unwrap_err().to_string().contains("环境不存在"));

        // 固定在 dev 的流程 + dev 是默认 → 重命名后引用全部跟着改
        workflow::save(root, None, "fx", &crate::config::Workflow {
            env: Some("dev".into()),
            steps: vec![crate::config::WorkflowStep { run: "collections/demo".into(), ..Default::default() }],
            ..Default::default()
        })
        .unwrap();
        rename(root, "dev", "local").unwrap();
        let d = doc(root);
        assert_eq!(d["defaultEnv"], "local");
        assert_eq!(d["workflows"]["fx"]["env"], "local");
        assert!(config::local_env_path(root, "local").exists() && !config::env_path(root, "dev").exists());
        assert!(rename(root, "local", "prod").unwrap_err().to_string().contains("已存在"));

        // 删除：默认环境、被固定的环境拒绝；普通环境连同本机覆盖一起删
        assert!(delete(root, "local").unwrap_err().to_string().contains("默认环境"));
        set_default(root, "prod").unwrap();
        assert_eq!(doc(root)["defaultEnv"], "prod");
        assert!(delete(root, "local").unwrap_err().to_string().contains("流程 fx"));
        workflow::delete(root, "fx").unwrap();
        delete(root, "local").unwrap();
        assert!(!config::env_path(root, "local").exists() && !config::local_env_path(root, "local").exists());
        let names: Vec<String> = list(root).unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["prod", "staging"]);
        assert!(set_default(root, "nope").is_err());
    }
}
