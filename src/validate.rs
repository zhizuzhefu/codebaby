use crate::config::{load_strict, Collection, EnvFile, Project, TestCase};
use crate::runner::discover::{self, DiscoveredCase};
use crate::script::{QuickJsHost, ScriptHost};
use crate::vars::{find_undefined, Vars};
use anyhow::Result;
use std::path::Path;

pub struct ValidationIssue {
    pub file: String,
    pub level: &'static str, // error / warn
    pub message: String,
}

/// 校验一套用例目录：
/// 1. 所有配置文件严格解析（serde deny_unknown_fields）
/// 2. {{var}} 引用存在性（含脚本写入的 vars）
/// 3. pre/post/tests 脚本语法预编译
pub fn validate_paths(paths: &[std::path::PathBuf], env_name: &str) -> Result<Vec<ValidationIssue>> {
    let mut issues = vec![];
    let host = QuickJsHost::new();

    // 项目与环境文件
    let roots: Vec<_> = paths.iter().map(|p| discover::find_project_root(p)).collect();
    let root0 = roots.iter().flatten().next().cloned();
    let mut base_vars = Vars::new();
    if let Some(root) = &root0 {
        if let Some(pf) = discover::project_file(root) {
            match load_strict::<Project>(&pf) {
                Ok(p) => base_vars.push_layer(p.vars.plain()),
                Err(e) => issues.push(ValidationIssue {
                    file: pf.display().to_string(),
                    level: "error",
                    message: format!("{e:#}"),
                }),
            }
        }
        let env_path = root.join("envs").join(format!("{env_name}.json"));
        if env_path.exists() {
            match load_strict::<EnvFile>(&env_path) {
                Ok(f) => base_vars.push_layer(f.vars.plain()),
                Err(e) => issues.push(ValidationIssue {
                    file: env_path.display().to_string(),
                    level: "error",
                    message: format!("{e:#}"),
                }),
            }
        }
    }

    // 各目录层级 collection.json
    for p in paths {
        check_collections_under(p, &mut issues);
    }

    // 用例：第一遍收集全部脚本写入的变量（跨用例传递），第二遍逐例校验
    let cases = discover::discover(paths)?;
    let mut global_written: Vec<String> = vec![];
    for dc in &cases {
        if let Ok(c) = load_strict::<TestCase>(&dc.path) {
            for n in scan_var_writes(&c.pre).into_iter().chain(scan_var_writes(&c.post)) {
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
        if let Err(e) = load_strict::<Collection>(entry.path()) {
            issues.push(ValidationIssue {
                file: entry.path().display().to_string(),
                level: "error",
                message: format!("{e:#}"),
            });
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
    let case = match load_strict::<TestCase>(&dc.path) {
        Ok(c) => c,
        Err(e) => {
            issues.push(ValidationIssue { file, level: "error", message: format!("{e:#}") });
            return;
        }
    };

    // 已知变量：base + 集合链 + 全部用例脚本写入的 vars（跨用例传递）
    let mut vars = base.clone();
    for c in &dc.chain {
        vars.push_layer(c.vars.plain());
    }
    let mut known = vars.env_snapshot();
    for k in global_written {
        known.entry(k.clone()).or_insert_with(String::new);
    }
    let known_vars = {
        let mut v = Vars::new();
        v.push_layer(known);
        v
    };

    // {{}} 引用检查
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
                message: format!("变量 {{{{{u}}}}} 无定义来源（env/collection/脚本写入均未发现）"),
            });
        }
    }

    // 脚本语法
    if !case.pre.is_empty() {
        if let Err(e) = host.compile_check(&case.pre) {
            issues.push(ValidationIssue { file: file.clone(), level: "error", message: format!("pre: {e:#}") });
        }
    }
    if !case.post.is_empty() {
        if let Err(e) = host.compile_check(&case.post) {
            issues.push(ValidationIssue { file: file.clone(), level: "error", message: format!("post: {e:#}") });
        }
    }
    for (i, t) in case.tests.iter().enumerate() {
        if let Err(e) = host.compile_check(&format!("return ({t})")) {
            issues.push(ValidationIssue {
                file: file.clone(),
                level: "error",
                message: format!("tests[{i}]: {e:#}"),
            });
        }
    }
}

/// 手工扫描脚本写入的 vars：匹配 vars.NAME = 或 vars["NAME"] =（无 regex 依赖）
fn scan_var_writes(src: &str) -> Vec<String> {
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
