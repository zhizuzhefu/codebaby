//! 导入器：OpenAPI 3 / Swagger 2 / Postman Collection v2.1 → requestbaby 项目
pub mod openapi;
pub mod postman;

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// curl 命令 → 单用例 JSON（支持 -X/-H/-d/--data/--data-raw 与位置 URL）
pub fn import_curl(cmd: &str) -> Result<serde_json::Value> {
    let tokens = shell_words(cmd)?;
    let mut method = String::new();
    let mut url = String::new();
    let mut headers: serde_json::Map<String, serde_json::Value> = Default::default();
    let mut data: Option<String> = None;
    let mut i = 0;
    let mut expect: Option<&str> = None;
    while i < tokens.len() {
        let t = &tokens[i];
        i += 1;
        if let Some(kind) = expect.take() {
            match kind {
                "method" => method = t.to_uppercase(),
                "url" => url = t.clone(),
                "header" => {
                    if let Some(idx) = t.find(':') {
                        headers.insert(
                            t[..idx].trim().to_string(),
                            serde_json::Value::String(t[idx + 1..].trim().to_string()),
                        );
                    }
                }
                "data" => data = Some(t.clone()),
                _ => {}
            }
            continue;
        }
        match t.as_str() {
            "curl" | "-s" | "-sS" | "-k" | "--insecure" | "-L" | "--compressed" | "-i" | "-v" | "--location" | "--silent" => {}
            "-X" | "--request" => expect = Some("method"),
            "-H" | "--header" => expect = Some("header"),
            "-d" | "--data" | "--data-raw" | "--data-binary" | "--data-ascii" | "--json" => {
                if t == "--json" {
                    headers
                        .entry("Content-Type".to_string())
                        .or_insert(serde_json::Value::String("application/json".into()));
                }
                expect = Some("data");
                if method.is_empty() {
                    method = "POST".into();
                }
            }
            "--url" => expect = Some("url"),
            other if !other.starts_with('-') && url.is_empty() => url = other.to_string(),
            _ => {}
        }
    }
    if url.is_empty() {
        anyhow::bail!("curl 命令缺少 URL");
    }
    if method.is_empty() {
        method = if data.is_some() { "POST".into() } else { "GET".into() };
    }
    let mut request = serde_json::json!({ "method": method, "url": url });
    if !headers.is_empty() {
        request["headers"] = Value::Object(headers);
    }
    if let Some(d) = data {
        request["body"] = match serde_json::from_str::<serde_json::Value>(&d) {
            Ok(v) => serde_json::json!({ "json": v }),
            Err(_) => serde_json::json!({ "text": d }),
        };
    }
    Ok(serde_json::json!({
        "name": "imported-from-curl",
        "request": request,
        "tests": ["res.status > 0"],
    }))
}

/// 极简 shell 分词（单引号/双引号/反斜杠转义）
fn shell_words(s: &str) -> Result<Vec<String>> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut has_token = false;
    for c in s.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    has_token = true;
                }
                '\\' => {
                    // 保留下一字符
                    cur.push('\\');
                    has_token = true;
                }
                c if c.is_whitespace() => {
                    if has_token || !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                        has_token = false;
                    }
                }
                c => {
                    cur.push(c);
                    has_token = true;
                }
            },
        }
    }
    if quote.is_some() {
        anyhow::bail!("引号未闭合");
    }
    if has_token {
        out.push(cur);
    }
    Ok(out)
}

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
