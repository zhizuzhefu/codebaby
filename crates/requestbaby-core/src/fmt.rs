//! 用例 JSON 规范形态：CLI `fmt` 与 Desktop 保存共用，保证两端写回的文件字节一致（Git 零噪音 diff）。

/// 字段规范排序与空字段清理
pub fn canonical_case(d: &serde_json::Value) -> serde_json::Value {
    let order = ["name", "tags", "skip", "expectStatus", "request", "grpc", "ws", "web", "android", "expect", "capture", "cases", "pre", "post", "tests"];
    let mut out = serde_json::Map::new();
    if let Some(obj) = d.as_object() {
        for key in order {
            if let Some(v) = obj.get(key) {
                let empty = v.is_null()
                    || v.as_str().map(|s| s.is_empty()).unwrap_or(false)
                    || v.as_array().map(|a| a.is_empty()).unwrap_or(false)
                    || v.as_bool() == Some(false)
                    || (matches!(key, "expect" | "capture") && v.as_object().is_some_and(|o| o.is_empty()));
                if !empty {
                    out.insert(key.into(), v.clone());
                }
            }
        }
        // 未知字段保留在尾部
        for (k, v) in obj {
            if !order.contains(&k.as_str()) {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    // expect 内部：status → headers → body（body 内按书写顺序，不重排）
    if let Some(exp) = out.get_mut("expect").and_then(|e| e.as_object_mut()) {
        let mut ordered = serde_json::Map::new();
        for key in ["status", "headers", "body"] {
            if let Some(v) = exp.get(key).filter(|v| !v.is_null() && !v.as_object().is_some_and(|o| o.is_empty())) {
                ordered.insert(key.into(), v.clone());
            }
        }
        for (k, v) in exp.iter() {
            if !ordered.contains_key(k) && !matches!(k.as_str(), "status" | "headers" | "body") {
                ordered.insert(k.clone(), v.clone());
            }
        }
        *exp = ordered;
    }
    // request 内部排序
    if let Some(req) = out.get_mut("request").and_then(|r| r.as_object_mut()) {
        let mut ordered = serde_json::Map::new();
        for key in ["method", "url", "query", "headers", "body", "timeoutMs", "retry", "insecure", "followRedirects"] {
            if let Some(v) = req.get(key) {
                if !(v.is_null() || v.as_object().map(|o| o.is_empty()).unwrap_or(false)) {
                    ordered.insert(key.into(), v.clone());
                }
            }
        }
        for (k, v) in req.iter() {
            if !ordered.contains_key(k) {
                ordered.insert(k.clone(), v.clone());
            }
        }
        *req = ordered;
    }
    // gRPC / WebSocket 内部排序；ws 步骤：动作在前，capture / timeoutMs 在后
    for (key, fields) in [
        ("grpc", &["target", "method", "message", "messages", "metadata", "timeoutMs", "insecure"][..]),
        ("ws", &["url", "query", "headers", "subprotocols", "steps", "timeoutMs", "insecure"][..]),
        ("web", &["url", "viewport", "timeoutMs", "steps"][..]),
        ("android", &["app", "activity", "apk", "clearData", "timeoutMs", "steps"][..]),
    ] {
        if let Some(v) = out.get_mut(key) {
            *v = reorder(v, fields);
        }
    }
    if let Some(steps) = out.get_mut("ws").and_then(|w| w.get_mut("steps")).and_then(|s| s.as_array_mut()) {
        for st in steps.iter_mut() {
            *st = reorder(st, &["send", "sendBinary", "receive", "waitFor", "wait", "close", "capture", "timeoutMs"]);
        }
    }
    // 界面步骤：动作在前，参数（value / clearData）其次，capture / timeoutMs / optional 在后
    const UI_STEP: [&str; 22] = [
        "goto", "click", "tap", "longPress", "hover", "fill", "select", "type", "press", "swipe", "back", "launch", "stop", "waitFor", "expect", "screenshot", "wait",
        "value", "clearData", "capture", "timeoutMs", "optional",
    ];
    for key in ["web", "android"] {
        if let Some(steps) = out.get_mut(key).and_then(|w| w.get_mut("steps")).and_then(|s| s.as_array_mut()) {
            for st in steps.iter_mut() {
                *st = reorder(st, &UI_STEP);
            }
        }
    }
    serde_json::Value::Object(out)
}

/// 按给定顺序重排对象键（未知键保留在尾部），去掉 null 与空对象
fn reorder(v: &serde_json::Value, fields: &[&str]) -> serde_json::Value {
    let Some(obj) = v.as_object() else { return v.clone() };
    let keep = |x: &serde_json::Value| !(x.is_null() || x.as_object().is_some_and(|o| o.is_empty()));
    let mut out = serde_json::Map::new();
    for f in fields {
        if let Some(x) = obj.get(*f) {
            // receive / waitFor 的 {} 有意义（只要收到一条消息）
            if keep(x) || matches!(*f, "receive" | "waitFor" | "message") {
                out.insert((*f).into(), x.clone());
            }
        }
    }
    for (k, x) in obj {
        if !out.contains_key(k) && keep(x) {
            out.insert(k.clone(), x.clone());
        }
    }
    serde_json::Value::Object(out)
}

/// 规范化后的文件文本（2 空格缩进 + 末尾换行）
pub fn canonical_case_text(d: &serde_json::Value) -> String {
    serde_json::to_string_pretty(&canonical_case(d)).unwrap_or_default() + "\n"
}

/// 项目级格式化结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FmtChange {
    /// 用例已重写为规范形态（check 模式下表示「可精简」）
    Case(std::path::PathBuf),
    /// 目录内公共 headers 已上提到 collection.json（check 模式不产生）
    HoistedHeaders(std::path::PathBuf),
    /// 按写法建议改写（--fix）：文件 + 应用的规则说明
    Upgraded(std::path::PathBuf, Vec<String>),
}

#[derive(Debug, Clone, Default)]
pub struct FmtReport {
    pub check: bool,
    pub changes: Vec<FmtChange>,
}

impl FmtReport {
    /// 需精简 / 已改写的用例数
    pub fn case_changes(&self) -> usize {
        self.changes.iter().filter(|c| matches!(c, FmtChange::Case(_) | FmtChange::Upgraded(..))).count()
    }
}

/// 格式化并精简用例（`requestbaby fmt` / Desktop「格式化项目」）：
/// 1. 目录内所有用例完全相同的 headers → 上提 collection.json（用例删除）
/// 2. 字段按可读顺序排列，清理默认值空字段
/// 3. `fix = true`：按写法建议改写（teach：JS 断言 → expect、post 取值 → capture、expectStatus → expect.status、
///    写死的域名 → {{baseUrl}}、多余的 Number()、流程已固定环境时删掉 setup 里的环境守卫）
///
/// `check = true` 时只检查不写入。
pub fn format_paths(paths: &[std::path::PathBuf], check: bool, fix: bool) -> anyhow::Result<FmtReport> {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    let cases = crate::runner::discover::discover(paths)?;
    let ctx = if fix {
        paths.iter().find_map(|p| crate::runner::discover::find_project_root(p)).map(|r| crate::teach::Context::load(&r)).unwrap_or_default()
    } else {
        Default::default()
    };
    let mut report = FmtReport { check, changes: vec![] };
    if fix {
        // 集合：setup 里的环境守卫
        for p in paths.iter().filter(|p| p.is_dir()) {
            for entry in walkdir::WalkDir::new(p).into_iter().filter_map(|e| e.ok()).filter(|e| e.file_name() == "collection.json") {
                let Some(doc) = std::fs::read_to_string(entry.path()).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else {
                    continue;
                };
                let dir = entry.path().parent().unwrap_or(std::path::Path::new("."));
                let (fixed, advice) = crate::teach::upgrade_collection(&doc, dir, &ctx);
                let applied: Vec<String> = advice.into_iter().filter(|a| a.fixable).map(|a| a.message).collect();
                if !applied.is_empty() {
                    if !check {
                        crate::fsx::write(entry.path(), canonical_collection_text(&fixed))?;
                    }
                    report.changes.push(FmtChange::Upgraded(entry.path().to_path_buf(), applied));
                }
            }
        }
    }
    let mut by_dir: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    for dc in cases {
        let dir = dc.path.parent().unwrap_or(std::path::Path::new(".")).to_path_buf();
        by_dir.entry(dir).or_default().push(dc.path);
    }

    for (dir, files) in &by_dir {
        let mut docs: Vec<(PathBuf, serde_json::Value)> = vec![];
        for f in files {
            if let Some(v) = std::fs::read_to_string(f).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()) {
                docs.push((f.clone(), v));
            }
        }
        if docs.is_empty() {
            continue;
        }
        // 公共 headers：所有用例 key+value 均相同
        let mut common: Option<serde_json::Map<String, serde_json::Value>> = None;
        for (_, d) in &docs {
            let h = d.pointer("/request/headers").and_then(|v| v.as_object()).cloned();
            match (&common, h) {
                // 只上提启用的字符串值：停用行（{"disabled":true}）是用例自己的调试状态
                (None, Some(h)) if !h.is_empty() => common = Some(h.into_iter().filter(|(_, v)| v.is_string()).collect()),
                (Some(c), Some(h)) => {
                    common = Some(c.iter().filter(|(k, v)| h.get(*k) == Some(v)).map(|(k, v)| (k.clone(), v.clone())).collect());
                }
                _ => common = Some(Default::default()),
            }
        }
        let common = common.unwrap_or_default();

        for (f, d) in docs.iter_mut() {
            let before = serde_json::to_string_pretty(d).unwrap_or_default();
            let mut applied: Vec<String> = vec![];
            if fix {
                let (fixed, advice) = crate::teach::upgrade_case(d, &ctx);
                applied = advice.into_iter().filter(|a| a.fixable).map(|a| a.message).collect();
                if !applied.is_empty() {
                    *d = fixed;
                }
            }
            if !common.is_empty() {
                if let Some(h) = d.pointer_mut("/request/headers").and_then(|v| v.as_object_mut()) {
                    for (k, v) in &common {
                        if h.get(k) == Some(v) {
                            h.remove(k);
                        }
                    }
                    if h.is_empty() {
                        if let Some(req) = d.pointer_mut("/request").and_then(|r| r.as_object_mut()) {
                            req.remove("headers");
                        }
                    }
                }
            }
            let ordered = canonical_case(d);
            let after = serde_json::to_string_pretty(&ordered).unwrap_or_default();
            if before != after {
                if !check {
                    crate::fsx::write(&f, after + "\n")?;
                }
                report.changes.push(if applied.is_empty() { FmtChange::Case(f.clone()) } else { FmtChange::Upgraded(f.clone(), applied) });
            }
        }
        if !common.is_empty() && !check {
            let cf = dir.join("collection.json");
            let mut col: serde_json::Value = std::fs::read_to_string(&cf)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_else(|| serde_json::json!({}));
            if let Some(slot) = col.as_object_mut() {
                let hdrs = slot.entry("headers").or_insert_with(|| serde_json::Value::Object(Default::default()));
                if let Some(hm) = hdrs.as_object_mut() {
                    for (k, v) in &common {
                        hm.entry(k.clone()).or_insert_with(|| v.clone());
                    }
                }
            }
            crate::fsx::write(&cf, serde_json::to_string_pretty(&col).unwrap_or_default() + "\n")?;
            report.changes.push(FmtChange::HoistedHeaders(cf));
        }
    }
    Ok(report)
}

/// 按给定字段顺序重排对象并清理空值（集合 / 环境 / 项目配置共用）
fn ordered(d: &serde_json::Value, order: &[&str]) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    if let Some(obj) = d.as_object() {
        for key in order {
            if let Some(v) = obj.get(*key) {
                let empty = v.is_null()
                    || v.as_str().is_some_and(|s| s.is_empty())
                    || v.as_array().is_some_and(|a| a.is_empty())
                    || v.as_object().is_some_and(|o| o.is_empty());
                if !empty {
                    out.insert((*key).into(), v.clone());
                }
            }
        }
        for (k, v) in obj {
            if !order.contains(&k.as_str()) {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    serde_json::Value::Object(out)
}

fn pretty(v: &serde_json::Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default() + "\n"
}

/// collection.json 规范文本
pub fn canonical_collection_text(d: &serde_json::Value) -> String {
    pretty(&ordered(d, &["name", "tags", "headers", "vars", "setup", "teardown"]))
}

/// envs/<name>.json 规范文本（vars 保留空对象）
pub fn canonical_env_text(d: &serde_json::Value) -> String {
    let mut v = d.clone();
    if let Some(o) = v.as_object_mut() {
        o.entry("vars").or_insert_with(|| serde_json::json!({}));
    }
    pretty(&v)
}

/// requestbaby.json 规范文本
pub fn canonical_project_text(d: &serde_json::Value) -> String {
    let mut v = ordered(d, &["version", "defaultEnv", "vars", "timeoutMs", "cookies", "proxy", "grpc", "web", "android", "workflows"]);
    if let Some(o) = v.as_object_mut() {
        o.entry("version").or_insert(serde_json::json!(1));
    }
    pretty(&v)
}
