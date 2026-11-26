//! 用例 JSON 规范形态：CLI `fmt` 与 Desktop 保存共用，保证两端写回的文件字节一致（Git 零噪音 diff）。

/// 字段规范排序与空字段清理
pub fn canonical_case(d: &serde_json::Value) -> serde_json::Value {
    let order = ["name", "tags", "skip", "expectStatus", "request", "grpc", "ws", "regions", "web", "android", "desktop", "expect", "capture", "cases", "pre", "post", "tests"];
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
        ("web", &["url", "viewport", "timeout", "timeoutMs", "steps", "onFail"][..]),
        ("android", &["app", "activity", "apk", "clearData", "attach", "timeout", "timeoutMs", "steps", "onFail"][..]),
        ("desktop", &["app", "window", "restart", "timeout", "timeoutMs", "steps", "onFail"][..]),
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
    // 界面用例：别名统一成一种规范写法，步骤里动作在前、参数其次、capture / timeout / optional 在后
    for key in ["web", "android", "desktop"] {
        if let Some(spec) = out.get_mut(key) {
            canonical_ui_spec(spec, key);
        }
    }
    serde_json::Value::Object(out)
}

/// 界面步骤的字段顺序
const UI_STEP: [&str; 25] = [
    "goto", "click", "tap", "longPress", "hover", "fill", "select", "type", "press", "swipe", "back", "launch", "stop", "waitFor", "expect", "screenshot", "wait", "repeat", "if", "break", "notify",
    "value", "clearData", "capture", "timeout",
];

/// 界面用例（web / android / desktop）的规范写法，同一件事只留一种：
/// - 点击：Web / Mac 桌面写 `click`，Android 写 `tap`
/// - 时长写人话：`timeout`（不写 timeoutMs）、`repeat.for`（不写 forMs）、`wait: "2.5s"` / `"3s-8s"`（不写毫秒数 / `{min, max}`）
fn canonical_ui_spec(spec: &mut serde_json::Value, kind: &str) {
    let Some(o) = spec.as_object_mut() else { return };
    duration_key(o, "timeoutMs", "timeout");
    for key in ["steps", "onFail"] {
        if let Some(steps) = o.get_mut(key).and_then(|s| s.as_array_mut()) {
            steps.iter_mut().for_each(|st| canonical_ui_step(st, kind));
        }
    }
    let fields: &[&str] = match kind {
        "web" => &["url", "viewport", "timeout", "steps", "onFail"],
        "android" => &["app", "activity", "apk", "clearData", "attach", "timeout", "steps", "onFail"],
        _ => &["app", "window", "restart", "timeout", "steps", "onFail"],
    };
    *spec = reorder(spec, fields);
}

fn canonical_ui_step(step: &mut serde_json::Value, kind: &str) {
    use serde_json::Value;
    let Some(o) = step.as_object_mut() else { return };
    let (want, other) = if kind == "android" { ("tap", "click") } else { ("click", "tap") };
    if !o.contains_key(want) {
        if let Some(v) = o.remove(other) {
            o.insert(want.into(), v);
        }
    }
    duration_key(o, "timeoutMs", "timeout");
    let wait = match o.get("wait") {
        Some(Value::Number(n)) => n.as_u64().map(crate::config::ui::format_duration_ms),
        Some(Value::Object(r)) => match (r.get("min").and_then(Value::as_u64), r.get("max").and_then(Value::as_u64), r.len()) {
            (Some(a), Some(b), 2) => Some(format!("{}-{}", crate::config::ui::format_duration_ms(a), crate::config::ui::format_duration_ms(b))),
            _ => None,
        },
        _ => None,
    };
    if let Some(t) = wait {
        o.insert("wait".into(), Value::String(t));
    }
    if let Some(r) = o.get_mut("repeat").and_then(|r| r.as_object_mut()) {
        duration_key(r, "forMs", "for");
        if let Some(steps) = r.get_mut("steps").and_then(|s| s.as_array_mut()) {
            steps.iter_mut().for_each(|st| canonical_ui_step(st, kind));
        }
        let ordered = reorder(&Value::Object(std::mem::take(r)), &["for", "times", "cycle", "until", "var", "steps"]);
        *step.pointer_mut("/repeat").expect("repeat") = ordered;
    }
    if let Some(b) = step.get_mut("if").and_then(|b| b.as_object_mut()) {
        for key in ["then", "else"] {
            if let Some(steps) = b.get_mut(key).and_then(|s| s.as_array_mut()) {
                steps.iter_mut().for_each(|st| canonical_ui_step(st, kind));
            }
        }
        let ordered = reorder(&Value::Object(std::mem::take(b)), &["when", "then", "else"]);
        *step.pointer_mut("/if").expect("if") = ordered;
    }
    *step = reorder(step, &UI_STEP);
}

/// 数字毫秒的 `num_key`（timeoutMs / forMs）改成人话的 `text_key`（timeout / for）；已经写了人话就不动
fn duration_key(o: &mut serde_json::Map<String, serde_json::Value>, num_key: &str, text_key: &str) {
    use serde_json::Value;
    if o.contains_key(num_key) && o.contains_key(text_key) {
        return;
    }
    if let Some(v) = o.remove(num_key) {
        match &v {
            Value::Number(n) if n.as_u64().is_some() => {
                o.insert(text_key.into(), Value::String(crate::config::ui::format_duration_ms(n.as_u64().unwrap_or(0))));
            }
            // 写成文字的 timeoutMs（"15s"）也归到 timeout
            Value::String(_) => {
                o.insert(text_key.into(), v);
            }
            _ => {
                o.insert(num_key.into(), v);
            }
        }
    }
    // timeout / for 写了数字：也改成人话
    if let Some(ms) = o.get(text_key).and_then(Value::as_u64) {
        o.insert(text_key.into(), Value::String(crate::config::ui::format_duration_ms(ms)));
    }
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

/// 规范化后的文件文本（2 空格缩进 + 末尾换行）。
/// 界面用例（web / android / desktop）与 `regions` 里放得下的小对象 / 数组保持一行：一步一行才读得动，
/// 不然每个定位、每个 env 都被拆成好几行，几十行的监控用例会膨胀成几百行。接口 / gRPC / WebSocket 用例与标准美化输出一致
pub fn canonical_case_text(d: &serde_json::Value) -> String {
    render_case_text(&canonical_case(d))
}

/// 一行放不下就换行的宽度（含缩进）
const INLINE_WIDTH: usize = 100;

/// 把已规范化的用例写成文本：见 [`canonical_case_text`]
pub fn render_case_text(canon: &serde_json::Value) -> String {
    let mut out = String::new();
    write_node(canon, 0, false, 0, &mut out);
    out.push('\n');
    out
}

fn quoted(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

/// 单行写法：`{ "k": v, "k2": v2 }` / `[a, b]`
fn inline_text(v: &serde_json::Value) -> String {
    use serde_json::Value;
    match v {
        Value::Object(m) if m.is_empty() => "{}".into(),
        Value::Object(m) => format!("{{ {} }}", m.iter().map(|(k, x)| format!("{}: {}", quoted(k), inline_text(x))).collect::<Vec<_>>().join(", ")),
        Value::Array(a) if a.is_empty() => "[]".into(),
        Value::Array(a) => format!("[{}]", a.iter().map(inline_text).collect::<Vec<_>>().join(", ")),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// zone：在界面用例 / regions 里（小对象可以一行）；prefix：本行前面「"键": 」已占的宽度
fn write_node(v: &serde_json::Value, indent: usize, zone: bool, prefix: usize, out: &mut String) {
    use serde_json::Value;
    let pad = "  ".repeat(indent);
    let fits = |v: &Value| zone && indent >= 2 && inline_text(v).chars().count() + indent * 2 + prefix < INLINE_WIDTH;
    match v {
        Value::Object(m) if !m.is_empty() => {
            if fits(v) {
                out.push_str(&inline_text(v));
                return;
            }
            out.push_str("{\n");
            for (i, (k, x)) in m.iter().enumerate() {
                let key = quoted(k);
                out.push_str(&format!("{pad}  {key}: "));
                let child_zone = zone || (indent == 0 && matches!(k.as_str(), "web" | "android" | "desktop" | "regions"));
                write_node(x, indent + 1, child_zone, key.chars().count() + 2, out);
                out.push_str(if i + 1 < m.len() { ",\n" } else { "\n" });
            }
            out.push_str(&format!("{pad}}}"));
        }
        Value::Array(a) if !a.is_empty() => {
            // 两个以上的对象（步骤）一步一行，不挤在一起
            let stacked = a.len() >= 2 && a.iter().any(|x| x.is_object() || x.is_array());
            if !stacked && fits(v) {
                out.push_str(&inline_text(v));
                return;
            }
            out.push_str("[\n");
            for (i, x) in a.iter().enumerate() {
                out.push_str(&format!("{pad}  "));
                write_node(x, indent + 1, zone, 0, out);
                out.push_str(if i + 1 < a.len() { ",\n" } else { "\n" });
            }
            out.push_str(&format!("{pad}]"));
        }
        other => out.push_str(&inline_text(other)),
    }
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

/// 格式化并精简用例（`codebaby fmt` / Desktop「格式化项目」）：
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
            // 界面用例的版式也是规范的一部分（小对象一行）：内容没变、只是排版不同（如别的工具写成逐行缩进）也改回来
            let layout_only = before == after && ["web", "android", "desktop"].iter().any(|k| ordered.get(*k).is_some()) && std::fs::read_to_string(&f).is_ok_and(|t| t != render_case_text(&ordered));
            if before != after || layout_only {
                if !check {
                    crate::fsx::write(&f, render_case_text(&ordered))?;
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

/// codebaby.json 规范文本
pub fn canonical_project_text(d: &serde_json::Value) -> String {
    let mut v = ordered(d, &["version", "defaultEnv", "vars", "timeoutMs", "cookies", "proxy", "grpc", "web", "android", "desktop", "workflows"]);
    if let Some(o) = v.as_object_mut() {
        o.entry("version").or_insert(serde_json::json!(1));
    }
    pretty(&v)
}

#[cfg(test)]
mod ui_alias_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ui_aliases_become_one_canonical_form() {
        let d = json!({"name": "x", "android": {"app": "a", "timeoutMs": 15000, "steps": [
            {"click": {"text": "a"}, "timeoutMs": 2500},
            {"wait": 2500},
            {"wait": {"min": 3000, "max": 8000}},
            {"wait": "3s-8s"},
            {"repeat": {"steps": [{"click": {"text": "b"}, "wait": 500}], "cycle": {"d": ["a"]}, "forMs": 28800000, "times": 2}},
            {"if": {"else": [{"wait": 1000}], "then": [{"click": {"text": "c"}}], "when": {"text": "x"}}}
        ], "onFail": [{"notify": "n", "timeoutMs": 10000}]}});
        let c = canonical_case(&d);
        let a = &c["android"];
        assert_eq!(a["timeout"], "15s");
        assert!(a.get("timeoutMs").is_none());
        let steps = a["steps"].as_array().unwrap();
        assert_eq!(steps[0], json!({"tap": {"text": "a"}, "timeout": "2.5s"}), "Android 点击写 tap，毫秒数写人话");
        assert_eq!((&steps[1], &steps[2], &steps[3]), (&json!({"wait": "2.5s"}), &json!({"wait": "3s-8s"}), &json!({"wait": "3s-8s"})));
        // repeat：for 取代 forMs，字段按 for / times / cycle / until / var / steps 排；子步骤同样统一
        let r = steps[4]["repeat"].as_object().unwrap();
        assert_eq!(r.keys().collect::<Vec<_>>(), ["for", "times", "cycle", "steps"]);
        assert_eq!(r["for"], "8h");
        assert_eq!(r["steps"][0], json!({"tap": {"text": "b"}, "wait": "500ms"}));
        // if：when / then / else 顺序，分支里同样统一
        let b = steps[5]["if"].as_object().unwrap();
        assert_eq!(b.keys().collect::<Vec<_>>(), ["when", "then", "else"]);
        assert_eq!(b["then"][0], json!({"tap": {"text": "c"}}));
        assert_eq!(b["else"][0], json!({"wait": "1s"}));
        assert_eq!(a["onFail"][0], json!({"notify": "n", "timeout": "10s"}));
        // 规格内字段顺序：app → timeout → steps → onFail
        assert_eq!(a.as_object().unwrap().keys().collect::<Vec<_>>(), ["app", "timeout", "steps", "onFail"]);
        // 再格式化一遍不变（Git 零噪音）
        assert_eq!(canonical_case(&c), c);
    }

    #[test]
    fn web_and_desktop_use_click_and_already_canonical_text_is_untouched() {
        for kind in ["web", "desktop"] {
            let d = json!({"name": "x", kind: {"app": "a", "url": "u", "steps": [{"tap": {"text": "a"}}, {"click": {"text": "b"}, "timeout": "5s"}]}});
            let c = canonical_case(&d);
            assert_eq!(c[kind]["steps"][0], json!({"click": {"text": "a"}}), "{kind}");
            assert_eq!(c[kind]["steps"][1], json!({"click": {"text": "b"}, "timeout": "5s"}), "{kind}");
        }
        // timeoutMs 写成文字、timeout 写成数字，也归到 timeout 的人话写法
        let d = json!({"name": "x", "web": {"url": "u", "timeoutMs": "20s", "steps": [{"wait": 1, "timeout": 3000}]}});
        let c = canonical_case(&d);
        assert_eq!((&c["web"]["timeout"], &c["web"]["steps"][0]["timeout"]), (&json!("20s"), &json!("3s")));
        // 两种写法同时存在：不替用户做主，原样保留
        let d = json!({"name": "x", "web": {"url": "u", "timeoutMs": 1000, "timeout": "2s"}});
        let c = canonical_case(&d);
        assert_eq!((&c["web"]["timeoutMs"], &c["web"]["timeout"]), (&json!(1000), &json!("2s")));
    }

    #[test]
    fn api_ws_and_grpc_numbers_are_left_alone() {
        let d = json!({"name": "x",
            "request": {"method": "GET", "url": "{{baseUrl}}/a", "timeoutMs": 800},
            "ws": {"url": "ws://x", "timeoutMs": 500, "steps": [{"wait": 300, "timeoutMs": 100}]}});
        let c = canonical_case(&d);
        assert_eq!(c["request"]["timeoutMs"], 800);
        assert_eq!((&c["ws"]["timeoutMs"], &c["ws"]["steps"][0]["wait"], &c["ws"]["steps"][0]["timeoutMs"]), (&json!(500), &json!(300), &json!(100)));
    }

    #[test]
    fn ui_cases_keep_small_objects_on_one_line_and_other_protocols_are_unchanged() {
        let d = json!({"name": "x", "regions": {"条": {"x": 0, "y": 0.1, "w": 1, "h": 0.1}}, "android": {"app": "a", "steps": [
            {"tap": {"ocr": "{{date}}", "region": "条"}, "optional": true},
            {"wait": "2s"},
            {"repeat": {"for": "8h", "cycle": {"d": ["11-28", "11-29"]}, "steps": [{"wait": "1s"}, {"notify": "{{d}} 有号"}]}}
        ], "onFail": [{"notify": "监控中断"}]}});
        let text = canonical_case_text(&d);
        let want = r#"{
  "name": "x",
  "regions": {
    "条": { "x": 0, "y": 0.1, "w": 1, "h": 0.1 }
  },
  "android": {
    "app": "a",
    "steps": [
      { "tap": { "ocr": "{{date}}", "region": "条" }, "optional": true },
      { "wait": "2s" },
      {
        "repeat": {
          "for": "8h",
          "cycle": { "d": ["11-28", "11-29"] },
          "steps": [
            { "wait": "1s" },
            { "notify": "{{d}} 有号" }
          ]
        }
      }
    ],
    "onFail": [{ "notify": "监控中断" }]
  }
}
"#;
        assert_eq!(text, want);
        // 文本读回来就是规范化后的内容，再渲染一遍不变（Git 零噪音）
        let back: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(back, canonical_case(&d));
        assert_eq!(canonical_case_text(&back), text);
        // 一行放不下的步骤换行，不会拉出超长的行
        let long = json!({"name": "x", "web": {"url": "u", "steps": [{"fill": {"label": "这是一个很长很长很长很长的标签名字用来撑满一整行的宽度"}, "value": "{{一个同样很长很长很长的变量名字}}", "timeout": "15s"}]}});
        let text = canonical_case_text(&long);
        assert!(text.lines().all(|l| l.chars().count() <= INLINE_WIDTH + 12), "{text}");
        assert_eq!(serde_json::from_str::<serde_json::Value>(&text).unwrap(), canonical_case(&long));
        // 接口 / WebSocket 用例：与标准美化输出完全一致
        let api = json!({"name": "x", "request": {"method": "GET", "url": "{{baseUrl}}/a", "headers": {"A": "b"}}, "expect": {"status": 200}, "tags": ["a", "b"]});
        assert_eq!(canonical_case_text(&api), serde_json::to_string_pretty(&canonical_case(&api)).unwrap() + "\n");
        let ws = json!({"name": "x", "ws": {"url": "ws://x", "steps": [{"send": {"a": 1}}, {"receive": {}}]}});
        assert_eq!(canonical_case_text(&ws), serde_json::to_string_pretty(&canonical_case(&ws)).unwrap() + "\n");
    }

    #[test]
    fn durations_format_back_to_the_shortest_exact_text() {
        use crate::config::ui::{format_duration_ms, parse_duration_ms};
        for (ms, text) in [(0, "0s"), (500, "500ms"), (1000, "1s"), (2500, "2.5s"), (60_000, "60s"), (90_000, "90s"), (120_000, "2m"), (3_600_000, "1h"), (28_800_000, "8h"), (86_400_000, "1d"), (1234, "1234ms")] {
            assert_eq!(format_duration_ms(ms), text);
            assert_eq!(parse_duration_ms(text), Ok(ms), "{text} 要能读回来");
        }
    }
}
