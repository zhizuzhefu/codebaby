use crate::config::{load_strict, Collection, EnvFile, Project, TestCase};
use crate::runner::discover::{self, DiscoveredCase};
use crate::script::{QuickJsHost, ScriptHost};
use crate::vars::{find_undefined, VarVal, Vars};
use anyhow::Result;
use std::path::Path;

/// 校验问题（AI 可消费：file/level/rule/message 四元组）
#[derive(serde::Serialize, Clone, Debug)]
pub struct ValidationIssue {
    pub file: String,
    pub level: &'static str, // error / warn
    pub rule: &'static str,
    pub message: String,
}

pub const CASE_SCHEMA: &str = include_str!("../../../docs/schema/case.schema.json");
pub const COLLECTION_SCHEMA: &str = include_str!("../../../docs/schema/collection.schema.json");
pub const ENV_SCHEMA: &str = include_str!("../../../docs/schema/env.schema.json");
pub const PROJECT_SCHEMA: &str = include_str!("../../../docs/schema/project.schema.json");

fn schema_check(schema_json: &str, doc_path: &Path, doc_name: &str, issues: &mut Vec<ValidationIssue>) -> bool {
    let raw = match std::fs::read_to_string(doc_path) {
        Ok(r) => r,
        Err(_) => return false,
    };
    let doc: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            issues.push(ValidationIssue {
                file: doc_path.display().to_string(),
                level: "error",
                rule: "json-parse",
                message: format!("JSON 解析失败: {e}"),
            });
            return false;
        }
    };
    let schema: serde_json::Value = serde_json::from_str(schema_json).expect("内置 schema 合法");
    let compiled = jsonschema::JSONSchema::compile(&schema).expect("内置 schema 可编译");
    if let Err(errors) = compiled.validate(&doc) {
        for e in errors {
            issues.push(ValidationIssue {
                file: doc_path.display().to_string(),
                level: "error",
                rule: "schema",
                message: format!("[{doc_name}] {}", e),
            });
        }
        return false;
    }
    true
}

/// 校验一套用例目录：
/// 1. 四类配置文件 JSON Schema 结构校验（内置 schema，防漂移）
/// 2. serde 严格解析（deny_unknown_fields 兜底）
/// 3. {{var}} 引用存在性（含脚本写入、跨用例传递）
/// 4. pre/post/tests 脚本语法预编译
/// 5. workflow step.run 路径与 cases 数据文件存在性
pub fn validate_paths(paths: &[std::path::PathBuf], env_name: &str) -> Result<Vec<ValidationIssue>> {
    let mut issues = vec![];
    let host = QuickJsHost::new();

    let roots: Vec<_> = paths.iter().map(|p| discover::find_project_root(p)).collect();
    let root0 = roots.iter().flatten().next().cloned();
    let mut base_vars = Vars::new();
    if let Some(root) = &root0 {
        if let Some(pf) = discover::project_file(root) {
            if schema_check(PROJECT_SCHEMA, &pf, "project", &mut issues) {
                if let Ok(p) = load_strict::<Project>(&pf) {
                    base_vars.push_str_layer(p.vars.plain());
                    // workflow step.run 存在性
                    for (wf_name, wf) in &p.workflows {
                        for (i, step) in wf.steps.iter().enumerate() {
                            let target = root.join(&step.run);
                            if !target.exists() {
                                issues.push(ValidationIssue {
                                    file: pf.display().to_string(),
                                    level: "error",
                                    rule: "workflow-path",
                                    message: format!(
                                        "workflows.{wf_name}.steps[{i}].run 不存在: {}",
                                        step.run
                                    ),
                                });
                            }
                        }
                    }
                }
            }
        }
        let env_path = root.join("envs").join(format!("{env_name}.json"));
        if env_path.exists()
            && schema_check(ENV_SCHEMA, &env_path, "env", &mut issues) {
                if let Ok(f) = load_strict::<EnvFile>(&env_path) {
                    base_vars.push_str_layer(f.vars.plain());
                }
            }
    }

    // 各目录层级 collection.json
    for p in paths {
        check_collections_under(p, &mut issues);
    }

    // 用例
    let cases = discover::discover(paths)?;
    let mut global_written: Vec<String> = vec![];
    for dc in &cases {
        if let Ok(c) = load_strict::<TestCase>(&dc.path) {
            for n in scan_var_writes(&c.pre)
                .into_iter()
                .chain(scan_var_writes(&c.post))
            {
                if !global_written.contains(&n) {
                    global_written.push(n);
                }
            }
        }
    }
    for dc in &cases {
        validate_case(dc, &base_vars, &global_written, &host, &mut issues);
    }
    Ok(issues)
}

fn check_collections_under(p: &Path, issues: &mut Vec<ValidationIssue>) {
    let dir = if p.is_dir() { p.to_path_buf() } else { return };
    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name() == "collection.json")
    {
        if schema_check(COLLECTION_SCHEMA, entry.path(), "collection", issues) {
            if let Err(e) = load_strict::<Collection>(entry.path()) {
                issues.push(ValidationIssue {
                    file: entry.path().display().to_string(),
                    level: "error",
                    rule: "serde-parse",
                    message: format!("{e:#}"),
                });
            }
        }
    }
}

fn validate_case(
    dc: &DiscoveredCase,
    base: &Vars,
    global_written: &[String],
    host: &QuickJsHost,
    issues: &mut Vec<ValidationIssue>,
) {
    let file = dc.rel.clone();
    if !schema_check(CASE_SCHEMA, &dc.path, "case", issues) {
        return;
    }
    let case = match load_strict::<TestCase>(&dc.path) {
        Ok(c) => c,
        Err(e) => {
            issues.push(ValidationIssue {
                file,
                level: "error",
                rule: "serde-parse",
                message: format!("{e:#}"),
            });
            return;
        }
    };

    // cases 数据文件存在性
    if let Some(crate::config::CasesSpec::File { file: data_file }) = &case.cases {
        let p = dc.path.parent().unwrap_or(Path::new(".")).join(data_file);
        if !p.exists() {
            issues.push(ValidationIssue {
                file: file.clone(),
                level: "error",
                rule: "cases-file",
                message: format!("数据文件不存在: {data_file}"),
            });
        }
    }

    // 已知变量：base + 集合链 + 全部用例脚本写入（跨用例传递）
    let mut vars = base.clone();
    for c in &dc.chain {
        vars.push_str_layer(c.vars.plain());
    }
    let mut known = vars.env_snapshot();
    for k in global_written {
        known.entry(k.clone()).or_insert_with(|| VarVal::Str(String::new()));
    }
    let known_vars = {
        let mut v = Vars::new();
        v.push_layer(known);
        v
    };

    let mut texts = vec![case.request.url.clone()];
    texts.extend(case.request.headers.values().cloned());
    if let Some(b) = &case.request.body {
        texts.push(b.render());
    }
    for t in texts {
        for u in find_undefined(&t, &known_vars) {
            issues.push(ValidationIssue {
                file: file.clone(),
                level: "warn",
                rule: "undefined-var",
                message: format!("变量 {{{{{u}}}}} 无定义来源（env/collection/脚本写入均未发现）"),
            });
        }
    }

    if !case.pre.is_empty() {
        if let Err(e) = host.compile_check(&case.pre) {
            issues.push(ValidationIssue {
                file: file.clone(),
                level: "error",
                rule: "script-syntax",
                message: format!("pre: {e:#}"),
            });
        }
    }
    if !case.post.is_empty() {
        if let Err(e) = host.compile_check(&case.post) {
            issues.push(ValidationIssue {
                file: file.clone(),
                level: "error",
                rule: "script-syntax",
                message: format!("post: {e:#}"),
            });
        }
    }
    for (i, t) in case.tests.iter().enumerate() {
        if let Err(e) = host.compile_check(&format!("return ({t})")) {
            issues.push(ValidationIssue {
                file: file.clone(),
                level: "error",
                rule: "script-syntax",
                message: format!("tests[{i}]: {e:#}"),
            });
        }
    }
}

/// 手工扫描脚本写入的 vars：匹配 vars.NAME = 或 vars["NAME"] =（无 regex 依赖）
pub fn scan_var_writes(src: &str) -> Vec<String> {
    let mut names = vec![];
    let bytes = src.as_bytes();
    let mut i = 0;
    while let Some(pos) = src[i..].find("vars") {
        let at = i + pos;
        i = at + 4;
        // 前界检查：排除 myvars / a.vars 等误匹配
        if at > 0 {
            let prev = bytes[at - 1];
            if prev.is_ascii_alphanumeric() || prev == b'_' || prev == b'$' || prev == b'.' {
                continue;
            }
        }
        let mut j = at + 4;
        let name = if bytes.get(j) == Some(&b'.') {
            j += 1;
            let start = j;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            Some(src[start..j].to_string())
        } else if bytes.get(j) == Some(&b'[') && bytes.get(j + 1) == Some(&b'"') {
            j += 2;
            let start = j;
            while j < bytes.len() && bytes[j] != b'"' {
                j += 1;
            }
            Some(src[start..j].to_string())
        } else {
            None
        };
        if let Some(n) = name {
            if !n.is_empty() && !names.contains(&n) {
                names.push(n);
            }
        }
    }
    names
}

/// 在全项目上下文中校验单个文件：跨用例捕获的变量（如前序 post 写入的 token）不会误报未定义
pub fn validate_file(path: &Path, env_name: &str) -> Result<Vec<ValidationIssue>> {
    let Some(root) = discover::find_project_root(path) else {
        return validate_paths(&[path.to_path_buf()], env_name);
    };
    let all = validate_paths(&[root.join("collections")], env_name)?;
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    Ok(all
        .into_iter()
        // 问题路径相对项目根（与进程工作目录无关）
        .filter(|i| std::fs::canonicalize(root.join(&i.file)).map(|f| f == target).unwrap_or(false))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 项目不在进程工作目录下时，单文件校验仍能拿到该文件的问题（Desktop 从任意目录启动）
    #[test]
    fn validate_file_independent_of_cwd() {
        let dir = tempfile::tempdir().unwrap();
        crate::scaffold::init_project(dir.path()).unwrap();
        let case = dir.path().join("collections/demo/03-bad.json");
        std::fs::write(&case, r#"{"request":{"method":"GET","url":"{{baseUrl}}/{{nope}}"},"tests":["res.status &&"]}"#).unwrap();
        let issues = validate_file(&case, "dev").unwrap();
        let rules: Vec<&str> = issues.iter().map(|i| i.rule).collect();
        assert!(rules.contains(&"undefined-var"), "{rules:?}");
        assert!(rules.contains(&"script-syntax"), "{rules:?}");
        let clean = validate_file(&dir.path().join("collections/demo/01-health.json"), "dev").unwrap();
        assert!(clean.is_empty(), "{clean:?}");
    }
}
