//! 项目与文件脚手架：`requestbaby init` / `requestbaby new` 与 Desktop 的新建、重命名、删除共用。
use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};

/// 脚手架写入结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScaffoldEntry {
    pub path: PathBuf,
    /// false = 已存在而跳过
    pub created: bool,
}

const PROJECT: &str = "{\n  \"version\": 1,\n  \"defaultEnv\": \"dev\",\n  \"vars\": {},\n  \"timeoutMs\": 30000\n}\n";
const ENV_DEV: &str = "{\n  \"vars\": {\n    \"baseUrl\": \"http://localhost:8080\"\n  }\n}\n";
const ENV_PROD: &str = "{\n  \"vars\": {\n    \"baseUrl\": \"https://api.example.com\"\n  }\n}\n";
const DEMO_COLLECTION: &str = "{\n  \"name\": \"demo\",\n  \"tags\": [\"smoke\"],\n  \"vars\": {}\n}\n";
const DEMO_HEALTH: &str = r#"{
  "name": "health",
  "tags": ["smoke"],
  "request": {
    "method": "GET",
    "url": "{{baseUrl}}/health"
  },
  "tests": [
    "res.status === 200",
    "res.json().status === 'ok'"
  ]
}
"#;
const DEMO_LOGIN: &str = r#"{
  "name": "login",
  "tags": ["smoke", "auth"],
  "request": {
    "method": "POST",
    "url": "{{baseUrl}}/auth/login",
    "headers": { "Content-Type": "application/json" },
    "body": { "json": { "username": "admin", "password": "secret" } }
  },
  "post": "vars.token = res.json().token",
  "tests": [
    "res.status === 200",
    "vars.token.length > 0"
  ]
}
"#;
const GITIGNORE: &str = "# 运行历史（CLI / Desktop 共享，本地状态不入库）\n.requestbaby/\n";

/// 初始化项目脚手架（已存在的文件跳过，不覆盖）
pub fn init_project(dir: &Path) -> Result<Vec<ScaffoldEntry>> {
    let files: [(&str, &str); 7] = [
        ("requestbaby.json", PROJECT),
        ("envs/dev.json", ENV_DEV),
        ("envs/prod.json", ENV_PROD),
        ("collections/demo/collection.json", DEMO_COLLECTION),
        ("collections/demo/01-health.json", DEMO_HEALTH),
        ("collections/demo/02-login.json", DEMO_LOGIN),
        (".gitignore", GITIGNORE),
    ];
    let mut out = vec![];
    for (rel, content) in files {
        let path = dir.join(rel);
        out.push(ScaffoldEntry { created: write_new(&path, content)?, path });
    }
    Ok(out)
}

fn write_new(path: &Path, content: &str) -> Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("创建目录失败: {}", parent.display()))?;
    }
    std::fs::write(path, content).with_context(|| format!("写入失败: {}", path.display()))?;
    Ok(true)
}

/// 文件 / 目录名合法性：禁止路径分隔与保留名
pub fn check_name(name: &str) -> Result<()> {
    let n = name.trim();
    if n.is_empty() {
        bail!("名称不能为空");
    }
    if n.contains(['/', '\\']) || n == "." || n == ".." || n.starts_with('.') {
        bail!("名称不能包含路径分隔符或以 . 开头: {n}");
    }
    if matches!(n, "requestbaby.json" | "collection.json" | "envs") {
        bail!("{n} 是保留名称");
    }
    Ok(())
}

/// 把展示名转为文件名片段（保留中文，空白转 -）
pub fn slug(name: &str) -> String {
    let s: String = name
        .trim()
        .chars()
        .map(|c| if c.is_whitespace() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '-' } else { c })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "case".into()
    } else {
        s.to_lowercase()
    }
}

/// 目录内下一个两位序号前缀（文件名序 = 执行序）
fn next_prefix(dir: &Path) -> u32 {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let n = e.file_name().to_string_lossy().into_owned();
                    n.split('-').next().and_then(|p| p.parse::<u32>().ok())
                })
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0)
        + 1
}

/// 在集合目录下新建用例：`NN-<slug>.json`，返回文件路径
pub fn new_case(dir: &Path, name: &str, method: &str, url: &str) -> Result<PathBuf> {
    if name.trim().is_empty() {
        bail!("用例名不能为空");
    }
    std::fs::create_dir_all(dir)?;
    let file = dir.join(format!("{:02}-{}.json", next_prefix(dir), slug(name)));
    let doc = serde_json::json!({
        "name": name.trim(),
        "request": {
            "method": method.to_uppercase(),
            "url": if url.is_empty() { "{{baseUrl}}/" } else { url },
        },
        "tests": [],
    });
    if !write_new(&file, &crate::fmt::canonical_case_text(&doc))? {
        bail!("文件已存在: {}", file.display());
    }
    Ok(file)
}

/// 新建集合目录（含 collection.json）
pub fn new_collection(parent: &Path, name: &str) -> Result<PathBuf> {
    check_name(name)?;
    let dir = parent.join(name.trim());
    if dir.exists() {
        bail!("目录已存在: {}", dir.display());
    }
    let doc = serde_json::json!({ "name": name.trim() });
    write_new(&dir.join("collection.json"), &crate::fmt::canonical_collection_text(&doc))?;
    Ok(dir)
}

/// 新建环境文件 envs/<name>.json；from 给定时复制其变量
pub fn new_env(root: &Path, name: &str, from: Option<&str>) -> Result<PathBuf> {
    check_name(name)?;
    let path = root.join("envs").join(format!("{}.json", name.trim()));
    if path.exists() {
        bail!("环境已存在: {}", name.trim());
    }
    let doc = match from {
        Some(src) => {
            let text = std::fs::read_to_string(root.join("envs").join(format!("{src}.json")))
                .with_context(|| format!("读取环境 {src} 失败"))?;
            serde_json::from_str(&text).with_context(|| format!("环境 {src} 解析失败"))?
        }
        None => serde_json::json!({ "vars": {} }),
    };
    write_new(&path, &crate::fmt::canonical_env_text(&doc))?;
    Ok(path)
}

/// 重命名文件或目录（同级），返回新路径。用例文件自动保留 .json 扩展名
pub fn rename(path: &Path, new_name: &str) -> Result<PathBuf> {
    check_name(new_name)?;
    let parent = path.parent().ok_or_else(|| anyhow!("无法重命名根目录"))?;
    let mut name = new_name.trim().to_string();
    if path.is_file() && path.extension().is_some_and(|e| e == "json") && !name.ends_with(".json") {
        name.push_str(".json");
    }
    let target = parent.join(&name);
    if target.exists() {
        bail!("已存在同名文件: {name}");
    }
    std::fs::rename(path, &target).with_context(|| format!("重命名失败: {}", path.display()))?;
    Ok(target)
}

/// 删除用例文件或集合目录（目录递归删除）
pub fn delete(path: &Path) -> Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
    .with_context(|| format!("删除失败: {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_case_uses_next_prefix_and_canonical_form() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("01-a.json"), "{}").unwrap();
        std::fs::write(dir.path().join("07-b.json"), "{}").unwrap();
        let f = new_case(dir.path(), "Create Order", "post", "").unwrap();
        assert_eq!(f.file_name().unwrap(), "08-create-order.json");
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(v["request"]["method"], "POST");
        assert!(v.get("tests").is_none(), "空 tests 被规范化清理");
    }

    #[test]
    fn rename_keeps_json_extension_and_rejects_separators() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("01-a.json");
        std::fs::write(&f, "{}").unwrap();
        let n = rename(&f, "02-b").unwrap();
        assert_eq!(n.file_name().unwrap(), "02-b.json");
        assert!(rename(&n, "x/y").is_err());
    }
}
