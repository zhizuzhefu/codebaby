//! Postman Collection v2.1 → requestbaby
use super::{pretty, safe_name, write_file, ImportSummary};
use anyhow::Result;
use serde_json::{json, Value};
use std::path::Path;

pub fn import(doc: &Value, out: &Path, env_name: &str) -> Result<ImportSummary> {
    let mut sum = ImportSummary { out_dir: out.to_path_buf(), ..Default::default() };

    let name = doc
        .pointer("/info/name")
        .and_then(Value::as_str)
        .unwrap_or("postman")
        .to_string();

    write_file(
        &out.join("requestbaby.json"),
        &pretty(&json!({ "version": 1, "defaultEnv": env_name, "vars": {}, "timeoutMs": 15000 })),
    )?;
    write_file(&out.join(format!("envs/{env_name}.json")), &pretty(&json!({ "vars": { "baseUrl": "http://localhost:8080" } })))?;

    // collection 变量 → env vars
    if let Some(vars) = doc.get("variable").and_then(Value::as_array) {
        let mut m = serde_json::Map::new();
        for v in vars {
            if let Some(k) = v.get("key").and_then(Value::as_str) {
                let val = v.get("value");
                let val_str = match val {
                    Some(Value::String(s)) => s.clone(),
                    Some(Value::Null) | None => String::new(),
                    Some(other) => other.to_string(),
                };
                m.insert(k.to_string(), json!(val_str));
            }
        }
        write_file(
            &out.join(format!("envs/{env_name}.json")),
            &pretty(&json!({ "vars": m })),
        )?;
    }

    let mut idx = 0usize;
    if let Some(items) = doc.get("item").and_then(Value::as_array) {
        import_items(items, &out.join("collections"), &mut idx, &mut sum)?;
    }
    if sum.cases == 0 {
        anyhow::bail!("Postman Collection 未发现任何请求（item）");
    }
    let _ = name;
    Ok(sum)
}

fn import_items(items: &[Value], dir: &Path, idx: &mut usize, sum: &mut ImportSummary) -> Result<()> {
    for item in items {
        // 文件夹：递归
        if let Some(sub) = item.get("item").and_then(Value::as_array) {
            let name = item.get("name").and_then(Value::as_str).unwrap_or("folder");
            import_items(sub, &dir.join(safe_name(name)), idx, sum)?;
            continue;
        }
        // 请求
        let Some(req) = item.get("request") else { continue };
        *idx += 1;
        let item_name = item.get("name").and_then(Value::as_str).unwrap_or("request");
        let method = req
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("GET")
            .to_uppercase();

        // url：raw 或 PostmanUrl 对象
        let raw_url = match req.get("url") {
            Some(Value::String(s)) => s.clone(),
            Some(u) => u.get("raw").and_then(Value::as_str).unwrap_or("").to_string(),
            None => String::new(),
        };

        let mut headers = serde_json::Map::new();
        if let Some(hs) = req.get("header").and_then(Value::as_array) {
            for h in hs {
                if let (Some(k), Some(v)) = (
                    h.get("key").and_then(Value::as_str),
                    h.get("value").and_then(Value::as_str),
                ) {
                    headers.insert(k.to_string(), json!(v));
                }
            }
        }

        let mut body = None;
        if let Some(b) = req.get("body") {
            match b.get("mode").and_then(Value::as_str) {
                Some("raw") => {
                    let text = b.get("raw").and_then(Value::as_str).unwrap_or("");
                    let is_json = b
                        .pointer("/options/raw/language")
                        .and_then(Value::as_str)
                        .map(|l| l == "json")
                        .unwrap_or_else(|| text.trim_start().starts_with('{') || text.trim_start().starts_with('['));
                    // 尝试结构化 JSON（享受 {{var:number}} 等能力），失败则 text
                    body = Some(if is_json {
                        match serde_json::from_str::<Value>(text) {
                            Ok(v) => json!({ "json": v }),
                            Err(_) => json!({ "text": text }),
                        }
                    } else {
                        json!({ "text": text })
                    });
                }
                Some("urlencoded") => {
                    if let Some(arr) = b.get("urlencoded").and_then(Value::as_array) {
                        let pairs: Vec<String> = arr
                            .iter()
                            .filter_map(|e| {
                                Some(format!(
                                    "{}={}",
                                    e.get("key").and_then(Value::as_str)?,
                                    e.get("value").and_then(Value::as_str).unwrap_or("")
                                ))
                            })
                            .collect();
                        body = Some(json!({ "text": pairs.join("&") }));
                    }
                }
                _ => {}
            }
        }

        let mut request = json!({
            "method": method,
            "url": raw_url,
        });
        if !headers.is_empty() {
            request["headers"] = Value::Object(headers);
        }
        if let Some(b) = body {
            request["body"] = b;
        }

        let case = json!({
            "name": item_name,
            "request": request,
            "tests": ["res.status > 0"],
        });

        let file = format!("{:02}-{}", idx, safe_name(item_name));
        write_file(&dir.join(format!("{file}.json")), &pretty(&case))?;
        sum.cases += 1;

        // Postman 脚本（event）不迁移：pm.* API 不兼容
        if item.get("event").is_some() {
            sum.warnings
                .push(format!("{item_name}: 含 Postman 脚本（event），需手工迁移为 pre/post/tests"));
        }
    }
    Ok(())
}
