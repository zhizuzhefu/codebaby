use crate::config::{load_strict, Collection};
use anyhow::Result;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// 发现的用例：路径 + 集合链（浅 → 深）
#[derive(Debug, Clone)]
pub struct DiscoveredCase {
    pub path: PathBuf,
    /// 相对项目根的路径（决定执行序）
    pub rel: String,
    pub chain: Vec<Collection>,
}

const IGNORED_DIRS: &[&str] = &["envs", "node_modules", ".git", "target", ".idea", ".vscode"];

/// 从 start（文件取父目录）向上查找 apicase.json
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_dir() {
        start.to_path_buf()
    } else {
        start.parent()?.to_path_buf()
    };
    loop {
        if dir.join("apicase.json").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// 收集用例文件：目录递归 *.json，排除项目/集合/环境配置
fn collect_case_files(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut files = vec![];
    for p in paths {
        if p.is_file() {
            if is_case_file(p) {
                files.push(p.clone());
            }
            continue;
        }
        for entry in WalkDir::new(p)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                let name = e.file_name().to_string_lossy();
                !IGNORED_DIRS.contains(&name.as_ref()) || e.depth() == 0
            })
        {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => anyhow::bail!("遍历 {} 失败: {e}", p.display()),
            };
            if entry.file_type().is_file() && is_case_file(entry.path()) {
                files.push(entry.path().to_path_buf());
            }
        }
    }
    Ok(files)
}

fn is_case_file(p: &Path) -> bool {
    if p.extension().and_then(|e| e.to_str()) != Some("json") {
        return false;
    }
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    name != "apicase.json" && name != "collection.json" && !name.starts_with('.')
}

/// 发现 + 构建集合链 + 按 rel 排序
pub fn discover(paths: &[PathBuf]) -> Result<Vec<DiscoveredCase>> {
    let files = collect_case_files(paths)?;
    let mut cases = vec![];
    for f in files {
        let root = find_project_root(&f).unwrap_or_else(|| {
            f.parent().map(Path::to_path_buf).unwrap_or_default()
        });
        let rel = f
            .strip_prefix(&root)
            .unwrap_or(&f)
            .to_string_lossy()
            .replace('\\', "/");
        let chain = collection_chain(&root, f.parent().unwrap_or(&root));
        cases.push(DiscoveredCase { path: f, rel, chain });
    }
    cases.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(cases)
}

/// root → case 目录路径上每级 collection.json（浅 → 深，不含 root 自身）
fn collection_chain(root: &Path, case_dir: &Path) -> Vec<Collection> {
    let mut chain = vec![];
    let Ok(rel) = case_dir.strip_prefix(root) else {
        return chain;
    };
    let mut cur = root.to_path_buf();
    for seg in rel.components() {
        cur.push(seg);
        let cfg = cur.join("collection.json");
        if cfg.exists() {
            if let Ok(c) = load_strict::<Collection>(&cfg) {
                chain.push(c);
            }
        }
    }
    chain
}
