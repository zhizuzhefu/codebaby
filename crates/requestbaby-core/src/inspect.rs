//! 变量解析预览：某个用例在某个环境下最终会用到哪些变量、值是什么、来自哪一层。
//!
//! `requestbaby vars <case>` 与 Desktop「变量」视图共用。secret 值一律以 **** 展示。
use crate::config::{load_strict, Collection, EnvFile, Project, StringVars, TestCase};
use crate::runner::discover;
use crate::validate::scan_var_writes;
use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VarEntry {
    pub name: String,
    /// 最终值（secret 已脱敏）
    pub value: String,
    pub secret: bool,
    /// 生效来源：项目 / 环境 dev / 集合 core / 数据驱动
    pub source: String,
    /// 生效定义所在文件（相对项目根）：编辑入口
    pub defined_in: String,
    /// 被覆盖的低层来源（低 → 高）
    pub overridden: Vec<String>,
    /// 用例（含继承的集合 headers）是否引用了它
    pub used: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeVar {
    pub name: String,
    /// 写入它的用例 / 集合脚本（相对项目根）
    pub writers: Vec<String>,
    /// 运行时值会覆盖同名静态变量
    pub shadows_static: bool,
    pub used: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VarsPreview {
    pub env: String,
    pub entries: Vec<VarEntry>,
    pub runtime: Vec<RuntimeVar>,
    /// `{{%NAME}}` 引用的进程环境变量及是否已设置
    pub process_env: Vec<(String, bool)>,
    /// 引用了但任何层都没有定义（运行时会报「占位符未替换」）
    pub undefined: Vec<String>,
}

/// 用例引用的变量名：`{{name}}` / `{{name:number}}` 与脚本里的 `env.name`
fn referenced(texts: &[String]) -> (Vec<String>, Vec<String>) {
    let mut vars = vec![];
    let mut process = vec![];
    for t in texts {
        for inner in crate::runner::scan_leftover_placeholders(t) {
            if let Some(p) = inner.strip_prefix('%') {
                if !process.contains(&p.to_string()) {
                    process.push(p.to_string());
                }
                continue;
            }
            let name = inner.split(':').next().unwrap_or_default().trim().to_string();
            if !name.is_empty() && !vars.contains(&name) {
                vars.push(name);
            }
        }
        let bytes = t.as_bytes();
        let mut i = 0;
        while let Some(pos) = t[i..].find("env.") {
            let at = i + pos;
            i = at + 4;
            if at > 0 && (bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_' || bytes[at - 1] == b'.') {
                continue;
            }
            let start = at + 4;
            let mut j = start;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            let name = &t[start..j];
            if !name.is_empty() && !vars.iter().any(|v| v == name) {
                vars.push(name.to_string());
            }
        }
    }
    (vars, process)
}

pub fn case_vars(case_path: &Path, env: &str) -> Result<VarsPreview> {
    let root = discover::find_project_root(case_path).ok_or_else(|| anyhow!("未找到项目根（requestbaby.json）"))?;
    let case = load_strict::<TestCase>(case_path)?;
    let dc = discover::discover(&[case_path.to_path_buf()])?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("不是用例文件: {}", case_path.display()))?;

    // 低 → 高：项目 < 环境 < 集合链（浅 → 深）< 数据驱动首行；(来源, 文件, 变量)
    let rel = |p: &Path| p.strip_prefix(&root).unwrap_or(p).to_string_lossy().replace('\\', "/");
    let mut layers: Vec<(String, String, StringVars)> = vec![];
    if let Some(pf) = discover::project_file(&root) {
        layers.push(("项目".into(), rel(&pf), load_strict::<Project>(&pf).context("项目配置错误")?.vars));
    }
    let env_path = crate::config::env_path(&root, env);
    if env_path.exists() {
        let vars = load_strict::<EnvFile>(&env_path).with_context(|| format!("环境 {env} 解析失败"))?.vars;
        layers.push((format!("环境 {env}"), rel(&env_path), vars));
    }
    let local_path = crate::config::local_env_path(&root, env);
    if local_path.exists() {
        let vars = load_strict::<EnvFile>(&local_path).with_context(|| format!("{} 解析失败", rel(&local_path)))?.vars;
        layers.push((crate::varstore::VarScope::Local(env.to_string()).label(), rel(&local_path), vars));
    }
    for file in collection_files(&root, case_path) {
        let c = load_strict::<Collection>(&file).with_context(|| format!("{} 解析失败", rel(&file)))?;
        let dir = rel(file.parent().unwrap_or(&root));
        layers.push((crate::varstore::VarScope::Collection(dir).label(), rel(&file), c.vars));
    }
    let mut data_row: BTreeMap<String, String> = BTreeMap::new();
    if let Some(spec) = &case.cases {
        if let Ok(rows) = spec.resolve(case_path.parent().unwrap_or(Path::new("."))) {
            if let Some(first) = rows.into_iter().next() {
                data_row = first.into_iter().map(|(k, v)| (k, v.to_render())).collect();
            }
        }
    }

    let (used, process_refs) = case_references(&case, &dc.chain);

    let mut resolved: BTreeMap<String, VarEntry> = BTreeMap::new();
    let mut apply = |name: &str, value: String, secret: bool, source: &str, file: &str| {
        let shown = if secret { "****".to_string() } else { value };
        match resolved.get_mut(name) {
            Some(e) => {
                let prev = std::mem::replace(&mut e.source, source.to_string());
                e.overridden.push(prev);
                e.value = shown;
                e.secret = e.secret || secret;
                e.defined_in = file.to_string();
            }
            None => {
                resolved.insert(
                    name.to_string(),
                    VarEntry {
                        name: name.to_string(),
                        value: shown,
                        secret,
                        source: source.to_string(),
                        defined_in: file.to_string(),
                        overridden: vec![],
                        used: used.iter().any(|u| u == name),
                    },
                );
            }
        }
    };
    for (label, file, vars) in &layers {
        for (k, v) in vars.iter() {
            apply(k, v.value.clone(), v.secret, label, file);
        }
    }
    let data_label = "数据驱动（首行）";
    let case_file = rel(case_path);
    for (k, v) in &data_row {
        if k != "label" {
            apply(k, v.clone(), false, data_label, &case_file);
        }
    }

    let writers = runtime_writers(&root);
    let runtime: Vec<RuntimeVar> = writers
        .into_iter()
        .map(|(name, writers)| RuntimeVar {
            shadows_static: resolved.contains_key(&name),
            used: used.contains(&name),
            name,
            writers,
        })
        .collect();

    let undefined: Vec<String> = used
        .iter()
        .filter(|u| !resolved.contains_key(*u) && !runtime.iter().any(|r| &r.name == *u))
        .cloned()
        .collect();
    let process_env = process_refs.into_iter().map(|n| {
        let set = std::env::var_os(&n).is_some();
        (n, set)
    });

    let mut entries: Vec<VarEntry> = resolved.into_values().collect();
    // 被引用的排前面
    entries.sort_by(|a, b| b.used.cmp(&a.used).then(a.name.cmp(&b.name)));
    Ok(VarsPreview { env: env.to_string(), entries, runtime, process_env: process_env.collect(), undefined })
}

/// 用例引用的变量与进程环境变量（含继承的集合 headers）
pub(crate) fn case_references(case: &TestCase, chain: &[Collection]) -> (Vec<String>, Vec<String>) {
    let mut texts = case.referencing_texts();
    texts.extend(chain.iter().flat_map(|c| c.headers.values().map(|v| v.value().to_string())));
    referenced(&texts)
}

/// 重跑部分用例时补上它们依赖的前置用例：引用的运行时变量（如 token）由排在前面的用例捕获，则一并执行（递归）。
/// 返回相对项目根的文件，按执行序
pub fn with_prerequisites(root: &Path, files: &[String]) -> Vec<String> {
    let writers = runtime_writers(root);
    let all: BTreeMap<String, discover::DiscoveredCase> =
        discover::discover(&[root.join("collections")]).unwrap_or_default().into_iter().map(|d| (d.rel.clone(), d)).collect();
    // 集合 setup / teardown 失败记录的是 collection.json：重跑该集合下的全部用例
    let files: Vec<String> = files
        .iter()
        .flat_map(|f| match f.strip_suffix("collection.json") {
            Some(dir) => all.keys().filter(|k| k.starts_with(dir)).cloned().collect::<Vec<_>>(),
            None => vec![f.clone()],
        })
        .collect();
    let mut keep: std::collections::BTreeSet<String> = files.iter().cloned().collect();
    let mut queue: Vec<String> = files.clone();
    while let Some(f) = queue.pop() {
        let Some(dc) = all.get(&f) else { continue };
        let Ok(case) = load_strict::<TestCase>(&dc.path) else { continue };
        for var in case_references(&case, &dc.chain).0 {
            for w in writers.get(&var).into_iter().flatten() {
                // 只补排在它前面的用例（完整运行时也是它们先捕获）；集合 setup 随集合自动执行
                if all.contains_key(w) && w.as_str() < f.as_str() && keep.insert(w.clone()) {
                    queue.push(w.clone());
                }
            }
        }
    }
    keep.into_iter().collect()
}

/// 单发（send）前要先运行的用例：本用例引用的运行时变量（由前面的用例捕获，如 token / owner）
/// 既不在会话里、也没有静态定义时，补上排在它前面、最近的那个写入者（递归）。
/// 返回 (用例, 它补上的变量)，相对项目根、按执行序；没有缺的变量时为空
pub fn send_prerequisites(root: &Path, case_path: &Path, env: &str, known: &std::collections::BTreeSet<String>) -> Vec<(String, Vec<String>)> {
    // 相对路径找到的项目根可能是空路径（= 当前目录）
    let canon = |p: &Path| {
        let p = if p.as_os_str().is_empty() { Path::new(".") } else { p };
        std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
    };
    let root = canon(root);
    let Ok(rel) = canon(case_path).strip_prefix(&root).map(|r| r.to_string_lossy().replace('\\', "/")) else { return vec![] };
    let mut known = known.clone();
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut queue = vec![rel];
    while let Some(f) = queue.pop() {
        let Ok(preview) = case_vars(&root.join(&f), env) else { continue };
        for rv in preview.runtime.iter().filter(|r| r.used && !r.shadows_static) {
            if known.contains(&rv.name) {
                continue;
            }
            // 集合 setup 随集合自动执行，不用补
            let Some(w) = rv.writers.iter().filter(|w| w.as_str() < f.as_str() && !w.ends_with("collection.json")).max() else { continue };
            known.insert(rv.name.clone());
            let vars = out.entry(w.clone()).or_default();
            if vars.is_empty() {
                queue.push(w.clone());
            }
            vars.push(rv.name.clone());
        }
    }
    out.into_iter().collect()
}

/// 全项目脚本写入的运行时变量（vars.x，含集合 setup/teardown）→ 写入者（相对项目根）
pub(crate) fn runtime_writers(root: &Path) -> BTreeMap<String, Vec<String>> {
    let mut writers: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut add = |names: Vec<String>, rel: &str| {
        for n in names {
            let w = writers.entry(n).or_default();
            if !w.iter().any(|x| x == rel) {
                w.push(rel.to_string());
            }
        }
    };
    for other in discover::discover(&[root.join("collections")]).unwrap_or_default() {
        if let Ok(c) = load_strict::<TestCase>(&other.path) {
            add(c.var_writes(), &other.rel);
        }
    }
    for entry in walkdir::WalkDir::new(root.join("collections")).into_iter().flatten() {
        if entry.file_name() == "collection.json" {
            if let Ok(c) = load_strict::<Collection>(entry.path()) {
                let rel = entry.path().strip_prefix(root).unwrap_or(entry.path()).to_string_lossy().replace('\\', "/");
                add(scan_var_writes(&c.setup).into_iter().chain(scan_var_writes(&c.teardown)).collect(), &rel);
            }
        }
    }
    writers
}

/// 项目根 → 用例目录路径上每级存在的 collection.json（浅 → 深，与运行时继承顺序一致）
fn collection_files(root: &Path, case_path: &Path) -> Vec<std::path::PathBuf> {
    let dir = case_path.parent().unwrap_or(root);
    let Ok(rel) = dir.strip_prefix(root) else { return vec![] };
    let mut cur = root.to_path_buf();
    let mut out = vec![];
    for seg in rel.components() {
        cur.push(seg);
        let f = cur.join("collection.json");
        if f.exists() {
            out.push(f);
        }
    }
    out
}

/// 预览片段：模板文本按变量占位符拆分（与 `Vars::render` 同一语法）
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Segment {
    Text { text: String },
    /// 静态可解析：值已脱敏（secret → ****）
    Var { name: String, value: String, source: String, defined_in: String },
    /// 由前序用例 / 集合脚本在运行时写入
    Runtime { name: String, writers: Vec<String> },
    /// `{{%NAME}}` 进程环境变量
    Process { name: String, set: bool },
    Undefined { name: String },
}

impl VarsPreview {
    /// 把模板（URL / header 值等）拆成预览片段
    pub fn segments(&self, template: &str) -> Vec<Segment> {
        let mut out = vec![];
        let mut rest = template;
        let mut text = String::new();
        while let Some(start) = rest.find("{{") {
            let after = &rest[start + 2..];
            let Some(end) = after.find("}}") else { break };
            text.push_str(&rest[..start]);
            if !text.is_empty() {
                out.push(Segment::Text { text: std::mem::take(&mut text) });
            }
            let inner = after[..end].trim();
            out.push(self.segment(inner));
            rest = &after[end + 2..];
        }
        text.push_str(rest);
        if !text.is_empty() {
            out.push(Segment::Text { text });
        }
        out
    }

    fn segment(&self, inner: &str) -> Segment {
        if let Some(p) = inner.strip_prefix('%') {
            return Segment::Process { name: p.to_string(), set: std::env::var_os(p).is_some() };
        }
        let name = inner.split(':').next().unwrap_or_default().trim().to_string();
        // 运行时捕获优先级最高：静态值只是首个请求前的初值
        if let Some(r) = self.runtime.iter().find(|r| r.name == name) {
            return Segment::Runtime { name, writers: r.writers.clone() };
        }
        match self.entries.iter().find(|e| e.name == name) {
            Some(e) => Segment::Var { value: e.value.clone(), source: e.source.clone(), defined_in: e.defined_in.clone(), name },
            None => Segment::Undefined { name },
        }
    }

    /// 片段拼回字符串（CLI 展示用）：变量替换为值，无法静态解析的保留占位符
    pub fn render_preview(&self, template: &str) -> String {
        self.segments(template)
            .into_iter()
            .map(|s| match s {
                Segment::Text { text } => text,
                Segment::Var { value, .. } => value,
                Segment::Runtime { name, .. } | Segment::Undefined { name } => format!("{{{{{name}}}}}"),
                Segment::Process { name, .. } => format!("{{{{%{name}}}}}"),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 单发缺运行时变量：补上排在前面、最近的写入者（递归）；会话里已有或静态定义了就不补
    #[test]
    fn send_prerequisites_fill_missing_runtime_vars_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let w = |rel: &str, text: &str| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        w("requestbaby.json", r#"{"version": 1, "defaultEnv": "dev"}"#);
        w("envs/dev.json", r#"{"vars": {"baseUrl": "http://x"}}"#);
        let get = |url: &str, capture: &str| format!(r#"{{"request": {{"method": "GET", "url": "{url}"}}, "capture": {capture}}}"#);
        w("collections/a/01-登录.json", &get("{{baseUrl}}/login", r#"{"token": "token"}"#));
        w("collections/a/02-下单.json", &get("{{baseUrl}}/orders?t={{token}}", r#"{"orderId": "id"}"#));
        w("collections/a/03-详情.json", &get("{{baseUrl}}/orders/{{orderId}}?t={{token}}", "{}"));
        let target = root.join("collections/a/03-详情.json");
        let known = std::collections::BTreeSet::new();
        assert_eq!(
            send_prerequisites(root, &target, "dev", &known),
            vec![
                ("collections/a/01-登录.json".to_string(), vec!["token".to_string()]),
                ("collections/a/02-下单.json".to_string(), vec!["orderId".to_string()]),
            ]
        );
        // 会话里已有 token：只补下单（下单自己用的 token 也已有）
        let known: std::collections::BTreeSet<String> = ["token".to_string()].into();
        assert_eq!(send_prerequisites(root, &target, "dev", &known), vec![("collections/a/02-下单.json".to_string(), vec!["orderId".to_string()])]);
        // 环境里静态定义了：不补
        w("envs/dev.json", r#"{"vars": {"baseUrl": "http://x", "token": "t", "orderId": "1"}}"#);
        assert!(send_prerequisites(root, &target, "dev", &Default::default()).is_empty());
        // 第一个用例没有依赖
        assert!(send_prerequisites(root, &root.join("collections/a/01-登录.json"), "dev", &Default::default()).is_empty());
    }

    #[test]
    fn references_cover_placeholders_types_process_env_and_scripts() {
        let (vars, process) = referenced(&[
            "{{baseUrl}}/a/{{id:number}}?t={{%TOKEN}}".into(),
            "data.x === env.sku && myenv.no".into(),
        ]);
        assert_eq!(vars, vec!["baseUrl", "id", "sku"]);
        assert_eq!(process, vec!["TOKEN"]);
    }

    #[test]
    fn preview_segments_resolve_layers_runtime_and_undefined() {
        let dir = tempfile::tempdir().unwrap();
        crate::scaffold::init_project(dir.path()).unwrap();
        let case = dir.path().join("collections/demo/02-login.json");
        let p = case_vars(&case, "dev").unwrap();
        let base = p.entries.iter().find(|e| e.name == "baseUrl").unwrap();
        assert_eq!(base.defined_in, "envs/dev.json");
        let segs = p.segments("{{baseUrl}}/u/{{token}}?x={{nope}}&h={{%HOME}}");
        assert_eq!(segs[0], Segment::Var {
            name: "baseUrl".into(),
            value: "http://localhost:8080".into(),
            source: "环境 dev".into(),
            defined_in: "envs/dev.json".into(),
        });
        assert_eq!(segs[1], Segment::Text { text: "/u/".into() });
        assert!(matches!(&segs[2], Segment::Runtime { name, .. } if name == "token"));
        assert_eq!(segs[4], Segment::Undefined { name: "nope".into() });
        assert!(matches!(&segs[6], Segment::Process { name, .. } if name == "HOME"));
        assert_eq!(p.render_preview("{{baseUrl}}/a/{{token}}"), "http://localhost:8080/a/{{token}}");
    }
}
