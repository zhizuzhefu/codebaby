//! 更新检查的应用级偏好：是否自动检查、上次检查时间（每天最多自动检查一次）。
//! 与 recent.rs 同在应用配置目录，不属于任何项目。
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const INTERVAL_MS: u64 = 24 * 3600 * 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Prefs {
    /// 启动时自动检查新版本（缺省开启；环境变量 REQUESTBABY_NO_UPDATE_CHECK=1 也可关闭）
    #[serde(default = "yes")]
    pub auto: bool,
    #[serde(default)]
    pub last_check_ms: u64,
}

fn yes() -> bool {
    true
}

impl Default for Prefs {
    fn default() -> Self {
        Self { auto: true, last_check_ms: 0 }
    }
}

fn file() -> Option<PathBuf> {
    Some(crate::recent::app_dir()?.join("updates.json"))
}

impl Prefs {
    pub fn load() -> Self {
        file().and_then(|f| std::fs::read_to_string(f).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(f) = file() else { return };
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(t) = serde_json::to_string_pretty(self) {
            let _ = requestbaby_core::fsx::write(f, t);
        }
    }

    /// 启动时是否该自动检查
    pub fn auto_due(&self) -> bool {
        self.auto && !std::env::var("REQUESTBABY_NO_UPDATE_CHECK").is_ok_and(|v| !v.is_empty() && v != "0") && requestbaby_core::history::now_ms().saturating_sub(self.last_check_ms) >= INTERVAL_MS
    }
}
