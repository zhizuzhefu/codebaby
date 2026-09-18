//! 导入器：OpenAPI 3 / Swagger 2 / Postman Collection v2.1 → requestbaby 项目
pub mod openapi;
pub mod postman;

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// 自动识别格式并导入
pub fn import(spec_path: &Path, out_dir: &Path, env_name: &str) -> Result<ImportSummary> {
    let raw = std::fs::read_to_string(spec_path)
        .with_context(|| format!("读取失败: {}", spec_path.display()))?;
    let doc: Value = if spec_path.extension().and_then(|e| e.to_str()) == Some("yaml")
        || spec_path.extension().and_then(|e| e.to_str()) == Some("yml")
        || raw.trim_start().starts_with("openapi:")
        || raw.trim_start().starts_with("swagger:")
    {
        serde_yaml::from_str(&raw).context("YAML 解析失败")?
    } else {
        serde_json::from_str(&raw).context("JSON 解析失败")?
    };

    if doc.get("openapi").is_some() {
        openapi::import(&doc, out_dir, env_name, 3)
    } else if doc.get("swagger").is_some() {
        openapi::import(&doc, out_dir, env_name, 2)
    } else if doc.get("info").and_then(|i| i.get("schema")).is_some()
        || doc.get("item").is_some()
    {
        postman::import(&doc, out_dir, env_name)
    } else {
        Err(anyhow!("无法识别格式：需包含 openapi/swagger（API spec）或 item（Postman Collection）"))
    }
}

#[derive(Default)]
pub struct ImportSummary {
    pub cases: usize,
    pub collections: usize,
    pub out_dir: PathBuf,
    pub base_url: String,
    pub warnings: Vec<String>,
}

/// 文件名安全化：/ {: } 等替换
pub fn safe_name(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => c,
            _ => '-',
        })
        .collect()
}

pub fn write_file(path: &Path, content: &str) -> Result<()> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::write(path, content)?;
    Ok(())
}

pub fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}
