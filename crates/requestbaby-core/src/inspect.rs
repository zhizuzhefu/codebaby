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

    // 低 → 高：项目 < 环境 < 集合链（浅 → 深）< 数据驱动首行
    let mut layers: Vec<(String, StringVars)> = vec![];
    if let Some(pf) = discover::project_file(&root) {
        layers.push(("项目".into(), load_strict::<Project>(&pf).context("项目配置错误")?.vars));
    }
    let env_path = root.join("envs").join(format!("{env}.json"));
    if env_path.exists() {
        layers.push((format!("环境 {env}"), load_strict::<EnvFile>(&env_path).with_context(|| format!("环境 {env} 解析失败"))?.vars));
    }
    for c in &dc.chain {
        layers.push((format!("集合 {}", if c.name.is_empty() { "(未命名)" } else { c.name.as_str() }), c.vars.clone()));
    }
    let mut data_row: BTreeMap<String, String> = BTreeMap::new();
    if let Some(spec) = &case.cases {
        if let Ok(rows) = spec.resolve(case_path.parent().unwrap_or(Path::new("."))) {
            if let Some(first) = rows.into_iter().next() {
                data_row = first;
            }
        }
    }

    let mut texts = vec![case.request.url.clone(), case.pre.clone(), case.post.clone()];
    texts.extend(case.request.headers.values().cloned());
    texts.extend(dc.chain.iter().flat_map(|c| c.headers.values().cloned()));
    if let Some(b) = &case.request.body {
        texts.push(b.render());
    }
    texts.extend(case.tests.iter().cloned());
    let (used, process_refs) = referenced(&texts);

    let mut resolved: BTreeMap<String, VarEntry> = BTreeMap::new();
    let mut apply = |name: &str, value: String, secret: bool, source: &str| {
        let shown = if secret { "****".to_string() } else { value };
        match resolved.get_mut(name) {
            Some(e) => {
                let prev = std::mem::replace(&mut e.source, source.to_string());
                e.overridden.push(prev);
                e.value = shown;
                e.secret = e.secret || secret;
            }
            None => {
                resolved.insert(
                    name.to_string(),
                    VarEntry {
                        name: name.to_string(),
                        value: shown,
                        secret,
                        source: source.to_string(),
                        overridden: vec![],
                        used: used.iter().any(|u| u == name),
                    },
                );
            }
        }
    };
    for (label, vars) in &layers {
        for (k, v) in vars.iter() {
            apply(k, v.value.clone(), v.secret, label);
        }
    }
    let data_label = "数据驱动（首行）";
    for (k, v) in &data_row {
        if k != "label" {
            apply(k, v.clone(), false, data_label);
        }
    }

    // 运行时捕获：全项目脚本写入的 vars.x（含集合 setup/teardown）
    let mut writers: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for other in discover::discover(&[root.join("collections")]).unwrap_or_default() {
        if let Ok(c) = load_strict::<TestCase>(&other.path) {
            for n in scan_var_writes(&c.pre).into_iter().chain(scan_var_writes(&c.post)) {
                let w = writers.entry(n).or_default();
                if !w.contains(&other.rel) {
                    w.push(other.rel.clone());
                }
            }
        }
    }
    for entry in walkdir::WalkDir::new(root.join("collections")).into_iter().flatten() {
        if entry.file_name() == "collection.json" {
            if let Ok(c) = load_strict::<Collection>(entry.path()) {
                let rel = entry.path().strip_prefix(&root).unwrap_or(entry.path()).to_string_lossy().replace('\\', "/");
                for n in scan_var_writes(&c.setup).into_iter().chain(scan_var_writes(&c.teardown)) {
                    let w = writers.entry(n).or_default();
                    if !w.contains(&rel) {
                        w.push(rel.clone());
                    }
                }
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_cover_placeholders_types_process_env_and_scripts() {
        let (vars, process) = referenced(&[
            "{{baseUrl}}/a/{{id:number}}?t={{%TOKEN}}".into(),
            "data.x === env.sku && myenv.no".into(),
        ]);
        assert_eq!(vars, vec!["baseUrl", "id", "sku"]);
        assert_eq!(process, vec!["TOKEN"]);
    }
}
