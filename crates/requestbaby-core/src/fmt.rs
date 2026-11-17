//! 用例 JSON 规范形态：CLI `fmt` 与 Desktop 保存共用，保证两端写回的文件字节一致（Git 零噪音 diff）。

/// 字段规范排序与空字段清理
pub fn canonical_case(d: &serde_json::Value) -> serde_json::Value {
    let order = ["name", "tags", "skip", "expectStatus", "request", "pre", "post", "tests", "cases"];
    let mut out = serde_json::Map::new();
    if let Some(obj) = d.as_object() {
        for key in order {
            if let Some(v) = obj.get(key) {
                let empty = v.is_null()
                    || v.as_str().map(|s| s.is_empty()).unwrap_or(false)
                    || v.as_array().map(|a| a.is_empty()).unwrap_or(false)
                    || v.as_bool() == Some(false);
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
    // request 内部排序
    if let Some(req) = out.get_mut("request").and_then(|r| r.as_object_mut()) {
        let mut ordered = serde_json::Map::new();
        for key in ["method", "url", "headers", "body", "timeoutMs", "retry", "insecure", "followRedirects"] {
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
}

#[derive(Debug, Clone, Default)]
pub struct FmtReport {
    pub check: bool,
    pub changes: Vec<FmtChange>,
}

impl FmtReport {
    /// 需精简 / 已改写的用例数
    pub fn case_changes(&self) -> usize {
        self.changes.iter().filter(|c| matches!(c, FmtChange::Case(_))).count()
    }
}

/// 格式化并精简用例（`requestbaby fmt` / Desktop「格式化项目」）：
/// 1. 目录内所有用例完全相同的 headers → 上提 collection.json（用例删除）
/// 2. 字段按可读顺序排列，清理默认值空字段
///
/// `check = true` 时只检查不写入。
pub fn format_paths(paths: &[std::path::PathBuf], check: bool) -> anyhow::Result<FmtReport> {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    let cases = crate::runner::discover::discover(paths)?;
    let mut by_dir: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    for dc in cases {
        let dir = dc.path.parent().unwrap_or(std::path::Path::new(".")).to_path_buf();
        by_dir.entry(dir).or_default().push(dc.path);
    }

    let mut report = FmtReport { check, changes: vec![] };
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
                (None, Some(h)) if !h.is_empty() => common = Some(h),
                (Some(c), Some(h)) => {
                    common = Some(c.iter().filter(|(k, v)| h.get(*k) == Some(v)).map(|(k, v)| (k.clone(), v.clone())).collect());
                }
                _ => common = Some(Default::default()),
            }
        }
        let common = common.unwrap_or_default();

        for (f, d) in docs.iter_mut() {
            let before = serde_json::to_string_pretty(d).unwrap_or_default();
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
                    std::fs::write(&f, after + "\n")?;
                }
                report.changes.push(FmtChange::Case(f.clone()));
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
            std::fs::write(&cf, serde_json::to_string_pretty(&col).unwrap_or_default() + "\n")?;
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
    let mut v = ordered(d, &["version", "defaultEnv", "vars", "timeoutMs", "workflows"]);
    if let Some(o) = v.as_object_mut() {
        o.entry("version").or_insert(serde_json::json!(1));
    }
    pretty(&v)
}
