//! 最近打开的项目（应用级偏好之一；另一个是 updates.rs 的更新检查开关）。
//!
//! 不属于任何项目，不写入项目目录；存放在系统的应用配置目录：
//! macOS `~/Library/Application Support/codebaby/recent-projects.json`，其他平台 `$XDG_CONFIG_HOME/codebaby/`。
use std::path::{Path, PathBuf};

const MAX: usize = 10;

/// 应用级配置目录
pub fn app_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    Some(if cfg!(target_os = "macos") {
        home?.join("Library/Application Support/codebaby")
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| home.map(|h| h.join(".config")))?.join("codebaby")
    })
}

fn file() -> Option<PathBuf> {
    Some(app_dir()?.join("recent-projects.json"))
}

/// 最近项目（新 → 旧），已不存在的目录自动略过
pub fn load() -> Vec<PathBuf> {
    let Some(f) = file() else { return vec![] };
    std::fs::read_to_string(f)
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<PathBuf>>(&t).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.is_dir())
        .collect()
}

pub fn remember(root: &Path) {
    let Some(f) = file() else { return };
    let mut list = load();
    list.retain(|p| p != root);
    list.insert(0, root.to_path_buf());
    list.truncate(MAX);
    if let Some(dir) = f.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(&list) {
        let _ = codebaby_core::fsx::write(f, text);
    }
}
