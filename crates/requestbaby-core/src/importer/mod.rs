//! 导入器：OpenAPI 3 / Swagger 2 / Postman Collection v2.1 → requestbaby 项目
pub mod openapi;
pub mod postman;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// curl 命令 → 单用例 JSON（浏览器 DevTools「Copy as cURL」/ 文档示例 / 自己的 dry-run 输出均可）
///
/// 支持：-X、-H、-d/--data/--data-raw/--data-binary/--json（多次 -d 以 & 拼接）、--data-urlencode → form、
/// -F/--form/--form-string → multipart（@文件）、-G/--get（数据转查询参数）、-b/--cookie、-u/--user、
/// -k、-L、--max-time；反斜杠续行与 $'…' 引号
pub fn import_curl(cmd: &str) -> Result<serde_json::Value> {
    use serde_json::{json, Map};
    let tokens = shell_words(cmd)?;
    let mut method = String::new();
    let mut url = String::new();
    let mut headers: Map<String, Value> = Map::new();
    let mut data: Vec<String> = vec![];
    let mut json_flag = false;
    let mut form: Map<String, Value> = Map::new();
    let mut multipart: Map<String, Value> = Map::new();
    let (mut get, mut insecure, mut follow) = (false, false, false);
    let mut timeout_ms: Option<u64> = None;
    let mut it = tokens.into_iter().skip_while(|t| t == "curl").peekable();
    while let Some(t) = it.next() {
        // --opt=value 形态
        let (flag, inline) = match t.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (t.clone(), None),
        };
        let val = |it: &mut std::iter::Peekable<_>| -> Result<String> {
            inline.clone().or_else(|| Iterator::next(it)).ok_or_else(|| anyhow!("{flag} 缺少参数"))
        };
        match flag.as_str() {
            "-X" | "--request" => method = val(&mut it)?.to_uppercase(),
            "-H" | "--header" => {
                let h = val(&mut it)?;
                if let Some((k, v)) = h.split_once(':') {
                    headers.insert(k.trim().to_string(), json!(v.trim()));
                }
            }
            "-d" | "--data" | "--data-raw" | "--data-binary" | "--data-ascii" => data.push(val(&mut it)?),
            "--json" => {
                json_flag = true;
                data.push(val(&mut it)?);
            }
            "--data-urlencode" => {
                let v = val(&mut it)?;
                let (k, v) = v.split_once('=').unwrap_or(("", v.as_str()));
                form.insert(k.to_string(), json!(v));
            }
            "-F" | "--form" | "--form-string" => {
                let v = val(&mut it)?;
                let (k, v) = v.split_once('=').ok_or_else(|| anyhow!("-F 参数应为 name=value: {v}"))?;
                let field = match v.strip_prefix('@').filter(|_| flag != "--form-string") {
                    Some(spec) => {
                        // @path;type=x;filename=y
                        let mut parts = spec.split(';');
                        let mut f = json!({ "file": parts.next().unwrap_or("") });
                        for p in parts {
                            match p.split_once('=') {
                                Some(("type", t)) => f["type"] = json!(t),
                                Some(("filename", n)) => f["filename"] = json!(n),
                                _ => {}
                            }
                        }
                        f
                    }
                    None => json!(v),
                };
                multipart.insert(k.to_string(), field);
            }
            "-b" | "--cookie" => {
                let c = val(&mut it)?;
                if c.contains('=') {
                    headers.insert("Cookie".into(), json!(c));
                }
            }
            "-u" | "--user" => {
                use base64::Engine;
                let cred = base64::engine::general_purpose::STANDARD.encode(val(&mut it)?);
                headers.insert("Authorization".into(), json!(format!("Basic {cred}")));
            }
            "-A" | "--user-agent" => {
                headers.insert("User-Agent".into(), json!(val(&mut it)?));
            }
            "-e" | "--referer" => {
                headers.insert("Referer".into(), json!(val(&mut it)?));
            }
            "-m" | "--max-time" => timeout_ms = val(&mut it)?.parse::<f64>().ok().map(|s| (s * 1000.0) as u64),
            "--url" => url = val(&mut it)?,
            "-G" | "--get" => get = true,
            "-k" | "--insecure" => insecure = true,
            "-L" | "--location" => follow = true,
            // 带参数但与用例无关的选项：吞掉参数
            "-o" | "--output" | "-w" | "--write-out" | "--connect-timeout" | "--retry" | "-x" | "--proxy" | "-c"
            | "--cookie-jar" | "--resolve" | "--cacert" | "--cert" | "--key" => {
                val(&mut it)?;
            }
            other if !other.starts_with('-') && url.is_empty() => url = other.to_string(),
            _ => {}
        }
    }
    if url.is_empty() {
        bail!("curl 命令缺少 URL");
    }
    let mut request = json!({});
    let mut query: Map<String, Value> = Map::new();
    if get {
        // -G：-d 数据转查询参数
        for d in data.drain(..) {
            for pair in d.split('&').filter(|p| !p.is_empty()) {
                let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                query.insert(k.to_string(), json!(v));
            }
        }
        query.extend(std::mem::take(&mut form));
    }
    let has_body = !data.is_empty() || !form.is_empty() || !multipart.is_empty();
    if method.is_empty() {
        method = if has_body { "POST".into() } else { "GET".into() };
    }
    let ct = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-type")).map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()));
    let body = if !multipart.is_empty() {
        // libcurl 生成带 boundary 的 Content-Type，手写的会导致 boundary 不匹配
        if let Some((k, v)) = &ct {
            if v.starts_with("multipart/") {
                headers.remove(k);
            }
        }
        Some(json!({ "multipart": multipart }))
    } else if !form.is_empty() {
        Some(json!({ "form": form }))
    } else if !data.is_empty() {
        let d = data.join("&");
        let is_form = ct.as_ref().is_some_and(|(_, v)| v.starts_with("application/x-www-form-urlencoded"));
        let parsed = serde_json::from_str::<Value>(&d).ok().filter(|v| v.is_object() || v.is_array());
        match parsed {
            Some(v) if !is_form => {
                // json body 自动补 Content-Type，抄来的同值头去掉，用例更干净
                if let Some((k, v)) = &ct {
                    if v.starts_with("application/json") {
                        headers.remove(k);
                    }
                }
                Some(json!({ "json": v }))
            }
            _ => {
                if json_flag {
                    headers.entry("Content-Type").or_insert(json!("application/json"));
                }
                Some(json!({ "text": d }))
            }
        }
    } else {
        None
    };
    request["method"] = json!(method);
    request["url"] = json!(url);
    if !query.is_empty() {
        request["query"] = Value::Object(query);
    }
    if !headers.is_empty() {
        request["headers"] = Value::Object(headers);
    }
    if let Some(b) = body {
        request["body"] = b;
    }
    if let Some(t) = timeout_ms {
        request["timeoutMs"] = json!(t);
    }
    if insecure {
        request["insecure"] = json!(true);
    }
    if follow {
        request["followRedirects"] = json!(true);
    }
    Ok(json!({
        "name": "imported-from-curl",
        "request": request,
        "expect": { "status": 200 },
    }))
}

/// POSIX shell 分词：'…' 原样、"…" 内 \ 转义 " \ $ `、$'…' ANSI-C 转义、裸 \ 转义下一字符、\ 换行 = 续行
fn shell_words(s: &str) -> Result<Vec<String>> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut has_token = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                has_token = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(x) => cur.push(x),
                        None => bail!("引号未闭合"),
                    }
                }
            }
            '"' => {
                has_token = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(x @ ('"' | '\\' | '$' | '`')) => cur.push(x),
                            Some('\n') => {}
                            Some(x) => {
                                cur.push('\\');
                                cur.push(x);
                            }
                            None => bail!("引号未闭合"),
                        },
                        Some(x) => cur.push(x),
                        None => bail!("引号未闭合"),
                    }
                }
            }
            '$' if chars.peek() == Some(&'\'') => {
                chars.next();
                has_token = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some('\\') => match chars.next() {
                            Some('n') => cur.push('\n'),
                            Some('t') => cur.push('\t'),
                            Some('r') => cur.push('\r'),
                            Some('x') => {
                                let hex: String = (0..2).filter_map(|_| chars.next_if(|c| c.is_ascii_hexdigit())).collect();
                                if let Ok(b) = u8::from_str_radix(&hex, 16) {
                                    cur.push(b as char);
                                }
                            }
                            Some('u') => {
                                let hex: String = (0..4).filter_map(|_| chars.next_if(|c| c.is_ascii_hexdigit())).collect();
                                if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                                    cur.push(ch);
                                }
                            }
                            Some(x) => cur.push(x),
                            None => bail!("引号未闭合"),
                        },
                        Some(x) => cur.push(x),
                        None => bail!("引号未闭合"),
                    }
                }
            }
            '\\' => match chars.next() {
                Some('\n') | Some('\r') => {
                    chars.next_if_eq(&'\n');
                }
                Some(x) => {
                    cur.push(x);
                    has_token = true;
                }
                None => {}
            },
            c if c.is_whitespace() => {
                if has_token {
                    out.push(std::mem::take(&mut cur));
                    has_token = false;
                }
            }
            c => {
                cur.push(c);
                has_token = true;
            }
        }
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

    let summary = if doc.get("openapi").is_some() {
        openapi::import(&doc, out_dir, env_name, 3)
    } else if doc.get("swagger").is_some() {
        openapi::import(&doc, out_dir, env_name, 2)
    } else if doc.get("info").and_then(|i| i.get("schema")).is_some()
        || doc.get("item").is_some()
    {
        postman::import(&doc, out_dir, env_name)
    } else {
        Err(anyhow!("无法识别格式：需包含 openapi/swagger（API spec）或 item（Postman Collection）"))
    }?;
    crate::guide::write_agent_entry(out_dir)?;
    Ok(summary)
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
    crate::fsx::write(path, content)?;
    Ok(())
}

pub fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}
