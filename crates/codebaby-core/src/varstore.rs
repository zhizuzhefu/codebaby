//! 变量的定义层：读写项目 / 环境 / 集合三类文件中的 `vars`，以及全项目变量总览。
//!
//! `codebaby vars` / `vars set` / `vars unset` 与 Desktop「变量总览」、未定义变量一键添加共用。
//! 读取保留原始定义形态（普通 / secret / 进程环境变量），不在这里解析 `{"from":"env"}` 的值。
use crate::config::{load_strict, TestCase};
use crate::runner::discover;
use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 变量定义所在的层
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "name", rename_all = "camelCase")]
pub enum VarScope {
    /// codebaby.json（最低优先级）
    Project,
    /// envs/<name>.json
    Env(String),
    /// envs/<name>.local.json：本机覆盖（优先于 envs/<name>.json，不入库）
    Local(String),
    /// <dir>/collection.json（dir 相对项目根）
    Collection(String),
}

impl VarScope {
    pub fn file(&self, root: &Path) -> PathBuf {
        match self {
            VarScope::Project => root.join(discover::PROJECT_FILE),
            VarScope::Env(name) => crate::config::env_path(root, name),
            VarScope::Local(name) => crate::config::local_env_path(root, name),
            VarScope::Collection(dir) => root.join(dir).join("collection.json"),
        }
    }

    pub fn label(&self) -> String {
        match self {
            VarScope::Project => "项目".into(),
            VarScope::Env(name) => format!("环境 {name}"),
            VarScope::Local(name) => format!("本机 {name}"),
            VarScope::Collection(dir) => format!("集合 {}", dir.strip_prefix("collections/").unwrap_or(dir)),
        }
    }

    /// 文件（相对项目根）→ 层；不是变量文件时 None
    pub fn of_file(rel: &str) -> Option<Self> {
        if rel == discover::PROJECT_FILE {
            Some(VarScope::Project)
        } else if let Some(name) = rel.strip_prefix("envs/").and_then(|n| n.strip_suffix(crate::config::LOCAL_SUFFIX)) {
            (!name.contains('/')).then(|| VarScope::Local(name.to_string()))
        } else if let Some(name) = rel.strip_prefix("envs/").and_then(|n| n.strip_suffix(".json")) {
            (!name.contains('/')).then(|| VarScope::Env(name.to_string()))
        } else {
            rel.strip_suffix("/collection.json").map(|d| VarScope::Collection(d.to_string()))
        }
    }

    fn canonical(&self, doc: &Value) -> String {
        match self {
            VarScope::Project => crate::fmt::canonical_project_text(doc),
            VarScope::Env(_) | VarScope::Local(_) => crate::fmt::canonical_env_text(doc),
            VarScope::Collection(_) => crate::fmt::canonical_collection_text(doc),
        }
    }
}

/// 变量的原始定义
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
pub enum VarDef {
    Plain(String),
    /// 输出层一律脱敏
    Secret(String),
    /// 运行时读进程环境变量（None = 与变量同名）
    FromEnv(Option<String>),
}

impl VarDef {
    pub fn decode(v: &Value) -> Self {
        match v {
            Value::Object(o) if o.get("from").and_then(Value::as_str) == Some("env") => {
                VarDef::FromEnv(o.get("name").and_then(Value::as_str).map(String::from))
            }
            Value::Object(o) => {
                let value = o.get("value").map(text_of).unwrap_or_default();
                if o.get("secret").and_then(Value::as_bool).unwrap_or(false) {
                    VarDef::Secret(value)
                } else {
                    VarDef::Plain(value)
                }
            }
            other => VarDef::Plain(text_of(other)),
        }
    }

    pub fn encode(&self) -> Value {
        match self {
            VarDef::Plain(v) => Value::String(v.clone()),
            VarDef::Secret(v) => json!({ "value": v, "secret": true }),
            VarDef::FromEnv(None) => json!({ "from": "env" }),
            VarDef::FromEnv(Some(n)) => json!({ "from": "env", "name": n }),
        }
    }

    /// 展示用（secret 脱敏，进程环境变量显示为 $NAME）
    pub fn display(&self, var_name: &str) -> String {
        match self {
            VarDef::Plain(v) => v.clone(),
            VarDef::Secret(_) => "****".into(),
            VarDef::FromEnv(n) => format!("${}", n.as_deref().unwrap_or(var_name)),
        }
    }
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn load_doc(file: &Path) -> Result<Value> {
    if !file.exists() {
        return Ok(json!({}));
    }
    let text = std::fs::read_to_string(file).with_context(|| format!("读取失败: {}", file.display()))?;
    let doc: Value = serde_json::from_str(&text).with_context(|| format!("JSON 解析失败: {}", file.display()))?;
    if !doc.is_object() {
        bail!("{} 不是 JSON 对象", file.display());
    }
    Ok(doc)
}

/// 某层定义的全部变量（按文件中的顺序）
pub fn read(root: &Path, scope: &VarScope) -> Result<Vec<(String, VarDef)>> {
    let doc = load_doc(&scope.file(root))?;
    Ok(doc
        .get("vars")
        .and_then(Value::as_object)
        .map(|m| m.iter().map(|(k, v)| (k.clone(), VarDef::decode(v))).collect())
        .unwrap_or_default())
}

/// 某层变量表的原始 JSON（保留数字 / 布尔等原始类型，供编辑器原样写回）
pub fn read_raw(root: &Path, scope: &VarScope) -> Result<Map<String, Value>> {
    let doc = load_doc(&scope.file(root))?;
    Ok(doc.get("vars").and_then(Value::as_object).cloned().unwrap_or_default())
}

fn write_vars(root: &Path, scope: &VarScope, edit: impl FnOnce(&mut Map<String, Value>)) -> Result<PathBuf> {
    let file = scope.file(root);
    match scope {
        VarScope::Env(name) if !file.exists() => bail!("环境不存在: {name}（先 codebaby new env {name}）"),
        VarScope::Local(name) => {
            if !crate::config::env_path(root, name).exists() {
                bail!("环境不存在: {name}（本机覆盖只能叠加在已有环境上）");
            }
            crate::scaffold::ensure_local_env_ignored(root)?;
        }
        _ => {}
    }
    let mut doc = load_doc(&file)?;
    let obj = doc.as_object_mut().expect("load_doc 保证对象");
    let vars = obj.entry("vars").or_insert_with(|| Value::Object(Map::new()));
    if !vars.is_object() {
        *vars = Value::Object(Map::new());
    }
    edit(vars.as_object_mut().expect("刚确保为对象"));
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::fsx::write(&file, scope.canonical(&doc)).with_context(|| format!("写入失败: {}", file.display()))?;
    Ok(file)
}

/// 设置变量（已存在则覆盖，保留其在文件中的位置）
pub fn set(root: &Path, scope: &VarScope, name: &str, def: &VarDef) -> Result<PathBuf> {
    let name = name.trim();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        bail!("变量名只能包含字母、数字、下划线: {name:?}");
    }
    write_vars(root, scope, |vars| {
        vars.insert(name.to_string(), def.encode());
    })
}

/// 整体替换某层的变量表（Desktop 变量编辑器保存）。值必须是三种合法形态之一
pub fn write_all(root: &Path, scope: &VarScope, vars: Map<String, Value>) -> Result<PathBuf> {
    for (name, v) in &vars {
        let ok = match v {
            Value::Object(o) => o.keys().all(|k| matches!(k.as_str(), "value" | "secret" | "from" | "name")),
            Value::Array(_) => false,
            _ => true,
        };
        if !ok {
            bail!("变量 {name} 的定义不合法：只能是标量、{{\"value\",\"secret\"}} 或 {{\"from\":\"env\"}}");
        }
    }
    write_vars(root, scope, |m| *m = vars)
}

/// 删除变量；返回是否存在过
pub fn unset(root: &Path, scope: &VarScope, name: &str) -> Result<bool> {
    let mut existed = false;
    if scope.file(root).exists() {
        write_vars(root, scope, |vars| existed = vars.shift_remove(name).is_some())?;
    }
    Ok(existed)
}

// ------------------------------------------------------------------ 变量总览

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VarColumn {
    pub scope: VarScope,
    pub label: String,
    /// 相对项目根
    pub file: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VarRow {
    pub name: String,
    /// 与 columns 一一对应；None = 该层未定义
    pub cells: Vec<Option<VarDef>>,
    /// 运行时写入它的脚本（相对项目根）
    pub runtime_writers: Vec<String>,
    /// 引用它的用例（相对项目根）
    pub used_by: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VarsMatrix {
    /// 低 → 高：项目、各环境、各集合（仅含定义了变量的集合）
    pub columns: Vec<VarColumn>,
    pub rows: Vec<VarRow>,
}

impl VarsMatrix {
    /// 被引用但任何层（含运行时捕获）都没有定义的变量
    pub fn undefined(&self) -> impl Iterator<Item = &VarRow> {
        self.rows.iter().filter(|r| r.cells.iter().all(Option::is_none) && r.runtime_writers.is_empty())
    }
}

/// 全项目变量总览：每个变量在每一层的定义、运行时捕获、引用它的用例
pub fn matrix(root: &Path) -> Result<VarsMatrix> {
    let mut scopes = vec![VarScope::Project];
    // 每个环境后紧跟它的本机覆盖列（存在时）
    for env in crate::config::list_envs(root) {
        let has_local = crate::config::local_env_path(root, &env).exists();
        scopes.push(VarScope::Env(env.clone()));
        if has_local {
            scopes.push(VarScope::Local(env));
        }
    }
    let mut collection_scopes = vec![];
    for e in walkdir::WalkDir::new(root.join("collections")).sort_by_file_name().into_iter().flatten() {
        if e.file_name() == "collection.json" {
            let dir = e.path().parent().unwrap_or(root);
            let rel = rel_of(root, dir);
            let scope = VarScope::Collection(rel);
            if !read(root, &scope).unwrap_or_default().is_empty() {
                collection_scopes.push(scope);
            }
        }
    }
    scopes.extend(collection_scopes);

    let mut rows: BTreeMap<String, VarRow> = BTreeMap::new();
    let width = scopes.len();
    let mut columns = vec![];
    for (ix, scope) in scopes.iter().enumerate() {
        for (name, def) in read(root, scope)? {
            rows.entry(name.clone())
                .or_insert_with(|| empty_row(&name, width))
                .cells[ix] = Some(def);
        }
        columns.push(VarColumn { label: scope.label(), file: rel_of(root, &scope.file(root)), scope: scope.clone() });
    }

    for (name, writers) in crate::inspect::runtime_writers(root) {
        rows.entry(name.clone()).or_insert_with(|| empty_row(&name, width)).runtime_writers = writers;
    }
    for dc in discover::discover(&[root.join("collections")]).unwrap_or_default() {
        let Ok(case) = load_strict::<TestCase>(&dc.path) else { continue };
        for name in crate::inspect::case_references(&case, &dc.chain).0 {
            let r = rows.entry(name.clone()).or_insert_with(|| empty_row(&name, width));
            if !r.used_by.contains(&dc.rel) {
                r.used_by.push(dc.rel.clone());
            }
        }
    }
    Ok(VarsMatrix { columns, rows: rows.into_values().collect() })
}

fn empty_row(name: &str, width: usize) -> VarRow {
    VarRow { name: name.to_string(), cells: vec![None; width], runtime_writers: vec![], used_by: vec![] }
}

fn rel_of(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        crate::scaffold::init_project(dir.path()).unwrap();
        dir
    }

    #[test]
    fn set_unset_round_trip_preserves_kinds_and_order() {
        let dir = project();
        let root = dir.path();
        let dev = VarScope::Env("dev".into());
        set(root, &dev, "token", &VarDef::Secret("s3cr3t".into())).unwrap();
        set(root, &dev, "home", &VarDef::FromEnv(Some("HOME".into()))).unwrap();
        set(root, &dev, "baseUrl", &VarDef::Plain("http://x".into())).unwrap();
        let vars = read(root, &dev).unwrap();
        assert_eq!(vars[0], ("baseUrl".into(), VarDef::Plain("http://x".into())), "覆盖保留原位置");
        assert_eq!(vars[1].1, VarDef::Secret("s3cr3t".into()));
        assert_eq!(vars[2].1, VarDef::FromEnv(Some("HOME".into())));
        assert!(unset(root, &dev, "token").unwrap());
        assert!(!unset(root, &dev, "token").unwrap());
        assert!(set(root, &dev, "bad name", &VarDef::Plain(String::new())).is_err());
        assert!(set(root, &VarScope::Env("nope".into()), "a", &VarDef::Plain("1".into())).is_err());
    }

    #[test]
    fn collection_scope_creates_file_and_matrix_lists_layers() {
        let dir = project();
        let root = dir.path();
        let col = VarScope::Collection("collections/demo".into());
        set(root, &col, "baseUrl", &VarDef::Plain("http://col".into())).unwrap();
        set(root, &VarScope::Project, "region", &VarDef::Plain("cn".into())).unwrap();
        let m = matrix(root).unwrap();
        let labels: Vec<&str> = m.columns.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["项目", "环境 dev", "环境 prod", "集合 demo"]);
        let base = m.rows.iter().find(|r| r.name == "baseUrl").unwrap();
        assert!(base.cells[0].is_none());
        assert_eq!(base.cells[1], Some(VarDef::Plain("http://localhost:8080".into())));
        assert_eq!(base.cells[3], Some(VarDef::Plain("http://col".into())));
        assert_eq!(base.used_by.len(), 2, "两个示例用例都引用 baseUrl");
        let token = m.rows.iter().find(|r| r.name == "token").unwrap();
        assert_eq!(token.runtime_writers, ["collections/demo/02-login.json"]);
        assert_eq!(m.undefined().count(), 0);
        assert_eq!(VarScope::of_file("collections/demo/collection.json"), Some(col));
        assert_eq!(VarScope::of_file("envs/dev.json"), Some(VarScope::Env("dev".into())));
    }

    /// 本机覆盖：写入时自动 gitignore；运行时覆盖环境同名变量；不出现在环境列表里
    #[test]
    fn local_override_layer() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        crate::scaffold::init_project(root).unwrap();
        std::fs::write(root.join(".gitignore"), ".codebaby/\n").unwrap();
        assert!(set(root, &VarScope::Local("nope".into()), "t", &VarDef::Plain("x".into())).is_err(), "只能叠加在已有环境上");
        set(root, &VarScope::Local("dev".into()), "token", &VarDef::Secret("s3cr3t".into())).unwrap();
        set(root, &VarScope::Local("dev".into()), "baseUrl", &VarDef::Plain("http://127.0.0.1:7001".into())).unwrap();
        assert!(crate::scaffold::local_env_ignored(root));
        assert_eq!(crate::config::list_envs(root), vec!["dev".to_string(), "prod".to_string()]);
        let env = crate::config::load_env(root, "dev").unwrap().unwrap();
        assert_eq!(env.vars["baseUrl"].value, "http://127.0.0.1:7001");
        assert!(env.vars["token"].secret);
        let ctx = crate::runner::load_project_ctx(root, "dev").unwrap();
        assert!(ctx.secret_values.contains(&"s3cr3t".to_string()));
        let m = matrix(root).unwrap();
        let cols: Vec<String> = m.columns.iter().map(|c| c.file.clone()).collect();
        assert_eq!(&cols[..4], ["codebaby.json", "envs/dev.json", "envs/dev.local.json", "envs/prod.json"]);
        assert_eq!(VarScope::of_file("envs/dev.local.json"), Some(VarScope::Local("dev".into())));
    }
}
