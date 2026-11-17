//! OpenAPI 3.x / Swagger 2.0 → requestbaby
use super::{pretty, safe_name, write_file, ImportSummary};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;

pub fn import(doc: &Value, out: &Path, env_name: &str, version: u32) -> Result<ImportSummary> {
    let mut sum = ImportSummary { out_dir: out.to_path_buf(), ..Default::default() };

    // ---- baseUrl ----
    let base_url = if version == 3 {
        doc.pointer("/servers/0/url")
            .and_then(Value::as_str)
            .unwrap_or("http://localhost:8080")
            .to_string()
    } else {
        let host = doc.get("host").and_then(Value::as_str).unwrap_or("localhost:8080");
        let base_path = doc.get("basePath").and_then(Value::as_str).unwrap_or("");
        let scheme = doc.pointer("/schemes/0").and_then(Value::as_str).unwrap_or("https");
        format!("{scheme}://{host}{base_path}")
    };
    sum.base_url = base_url.clone();

    write_file(
        &out.join("requestbaby.json"),
        &pretty(&json!({ "version": 1, "defaultEnv": env_name, "vars": {}, "timeoutMs": 15000 })),
    )?;
    write_file(
        &out.join(format!("envs/{env_name}.json")),
        &pretty(&json!({ "vars": { "baseUrl": base_url } })),
    )?;
    // 鉴权占位模板（token 走外部环境变量，不落盘）
    write_file(
        &out.join(format!("envs/{env_name}.example.json")),
        &pretty(&json!({ "vars": {
            "baseUrl": base_url,
            "token": { "from": "env", "name": "RB_TOKEN", "secret": true }
        }})),
    )?;

    // ---- 解析 paths，按主 tag 分目录 ----
    let Some(paths) = doc.get("paths").and_then(Value::as_object) else {
        anyhow::bail!("spec 缺少 paths");
    };
    let mut groups: BTreeMap<String, Vec<(String, Value)>> = BTreeMap::new();
    for (path, item) in paths {
        let Some(ops) = item.as_object() else { continue };
        for (method, op) in ops {
            if !matches!(
                method.as_str(),
                "get" | "post" | "put" | "patch" | "delete" | "head" | "options"
            ) {
                continue;
            }
            let Some(op_map) = op.as_object() else { continue };
            let tag = op_map
                .get("tags")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(Value::as_str)
                .unwrap_or("default")
                .to_string();
            let (file_name, case) = build_case(method, path, item, op_map, doc, version, &mut sum);
            groups
                .entry(safe_name(&tag))
                .or_default()
                .push((file_name, case));
        }
    }

    for (tag, cases) in groups {
        let dir = out.join("collections").join(&tag);
        write_file(
            &dir.join("collection.json"),
            &pretty(&json!({ "name": tag, "headers": {}, "vars": {} })),
        )?;
        for (name, case) in cases {
            write_file(&dir.join(format!("{name}.json")), &pretty(&case))?;
            sum.cases += 1;
        }
        sum.collections += 1;
    }
    Ok(sum)
}

fn build_case(
    method: &str,
    path: &str,
    path_item: &Value,
    op: &serde_json::Map<String, Value>,
    doc: &Value,
    version: u32,
    sum: &mut ImportSummary,
) -> (String, Value) {
    let op_id = op
        .get("operationId")
        .and_then(Value::as_str)
        .map(String::from)
        .unwrap_or_else(|| format!("{}{}", method, path));
    let summary = op.get("summary").and_then(Value::as_str).unwrap_or_default();
    let method_upper = method.to_uppercase();

    // URL：path 参数 → {{param}}
    let mut url = path.to_string();
    let path_params = collect_params(path_item, op, "path");
    for p in &path_params {
        url = url.replace(&format!("{{{}}}", p.0), &format!("{{{{{}}}}}", p.0));
    }

    // query 参数 → ?a={{a}}（有 example 用 example）
    let queries = collect_params(path_item, op, "query");
    if !queries.is_empty() {
        let qs: Vec<String> = queries
            .iter()
            .map(|(k, ex)| format!("{k}={}", ex.clone().unwrap_or_else(|| format!("{{{{{k}}}}}"))))
            .collect();
        url.push('?');
        url.push_str(&qs.join("&"));
    }

    // body（content 下任意 */json 类型，如 text/json、application/json; charset=utf-8）
    let op_v = Value::Object(op.clone());
    let body_schema: Option<Value> = if version == 3 {
        let content = op_v.pointer("/requestBody/content").and_then(Value::as_object);
        let json_media = content.and_then(|c| {
            c.keys().find(|k| k.to_ascii_lowercase().contains("json"))
        });
        json_media
            .and_then(|k| {
                let ptr = format!("/requestBody/content/{}/schema", k.replace('/', "~1").replace('~', "~0").replace("~01", "~1"));
                op_v.pointer(&ptr).cloned()
            })
            .or_else(|| op_v.pointer("/requestBody/content/application~1json/example").cloned().map(|e| json!({"example": e})))
    } else {
        op.get("parameters").and_then(Value::as_array).and_then(|ps| {
            ps.iter()
                .find(|p| p.get("in").and_then(Value::as_str) == Some("body"))
                .and_then(|p| p.get("schema"))
                .cloned()
        })
    };
    let body_example = body_schema.map(|s| schema_to_example(&s, None, doc, version, 0));

    // 默认期望状态码：responses 第一个 2xx
    let expect: Option<Value> = op
        .get("responses")
        .and_then(Value::as_object)
        .and_then(|r| {
            let mut codes: Vec<u16> = r
                .keys()
                .filter_map(|k| k.parse::<u16>().ok())
                .filter(|c| (200..300).contains(c))
                .collect();
            codes.sort();
            codes.first().map(|c| json!(c))
        });

    let mut request = json!({
        "method": method_upper,
        "url": format!("{{{{baseUrl}}}}{url}"),
    });
    if matches!(method_upper.as_str(), "POST" | "PUT" | "PATCH") {
        if let Some(b) = body_example {
            request["body"] = json!({ "json": b });
        }
    }

    let mut case = json!({
        "name": format!("{method_upper} {}", if summary.is_empty() { &op_id } else { summary }),
        "tags": [safe_name(op.get("tags").and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_str).unwrap_or("default"))],
        "request": request,
        "tests": ["res.status > 0"],
    });
    if let Some(e) = expect {
        case["expectStatus"] = e;
    }
    if !path_params.is_empty() {
        sum.warnings.push(format!(
            "{op_id}: path 参数 {} 需在 env/collection vars 中赋值",
            path_params.iter().map(|p| p.0.clone()).collect::<Vec<_>>().join(",")
        ));
    }
    let file_name = safe_name(&format!("{method}-{}", url_path_slug(path)));
    (file_name, case)
}

fn url_path_slug(path: &str) -> String {
    path.trim_matches('/').replace('/', "-")
}

/// 收集参数 (name, example)
fn collect_params(
    path_item: &Value,
    op: &serde_json::Map<String, Value>,
    in_: &str,
) -> Vec<(String, Option<String>)> {
    let mut out: Vec<(String, Option<String>)> = vec![];
    let mut visit = |params: Option<&Value>| {
        if let Some(arr) = params.and_then(Value::as_array) {
            for p in arr {
                if p.get("in").and_then(Value::as_str) == Some(in_) {
                    if let Some(name) = p.get("name").and_then(Value::as_str) {
                        if out.iter().any(|(n, _)| n == name) {
                            continue;
                        }
                        let ex = p
                            .get("example")
                            .and_then(Value::as_str)
                            .map(String::from)
                            .or_else(|| {
                                p.pointer("/schema/example")
                                    .and_then(Value::as_str)
                                    .map(String::from)
                            });
                        out.push((name.to_string(), ex));
                    }
                }
            }
        }
    };
    visit(path_item.get("parameters"));
    visit(op.get("parameters"));
    out
}

/// schema → 示例 JSON：example 优先，递归 properties；占位符用字段名（数字带 :number 标注）
fn schema_to_example(
    schema: &Value,
    field: Option<&str>,
    doc: &Value,
    version: u32,
    depth: usize,
) -> Value {
    if depth > 6 {
        return json!("...");
    }
    if let Some(r) = schema.get("$ref").and_then(Value::as_str) {
        let name = r.rsplit('/').next().unwrap_or_default();
        let target = if version == 3 {
            doc.pointer(&format!("/components/schemas/{name}"))
        } else {
            doc.pointer(&format!("/definitions/{name}"))
        };
        return match target {
            Some(t) => schema_to_example(t, None, doc, version, depth + 1),
            None => json!({}),
        };
    }
    if let Some(ex) = schema.get("example").filter(|v| !v.is_null()) {
        return ex.clone();
    }
    if let Some(d) = schema.get("default").filter(|v| !v.is_null()) {
        return d.clone();
    }
    // enum 取第一个
    if let Some(e) = schema.get("enum").and_then(Value::as_array).and_then(|a| a.first()) {
        return e.clone();
    }
    let ph = |tag: &str| {
        json!(match field {
            Some(f) => format!("{{{{{f}{tag}}}}}"),
            None => format!("{{{{placeholder{tag}}}}}"),
        })
    };
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => {
            let mut obj = serde_json::Map::new();
            if let Some(props) = schema.get("properties").and_then(Value::as_object) {
                for (k, v) in props {
                    obj.insert(
                        k.clone(),
                        schema_to_example(v, Some(k), doc, version, depth + 1),
                    );
                }
            }
            Value::Object(obj)
        }
        Some("array") => {
            let item = schema
                .get("items")
                .map(|i| schema_to_example(i, None, doc, version, depth + 1))
                .unwrap_or(json!({}));
            json!([item])
        }
        Some("integer") | Some("number") => ph(":number"),
        Some("boolean") => ph(":bool"),
        _ => ph(""),
    }
}
