//! 项目与文件脚手架：`codebaby init` / `codebaby new` 与 Desktop 的新建、重命名、删除共用。
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
  "expect": {
    "status": 200,
    "body": {
      "status": "ok"
    }
  }
}
"#;
const DEMO_LOGIN: &str = r#"{
  "name": "login",
  "tags": ["smoke", "auth"],
  "request": {
    "method": "POST",
    "url": "{{baseUrl}}/auth/login",
    "body": { "json": { "username": "admin", "password": "secret" } }
  },
  "expect": {
    "status": 200,
    "body": {
      "token": { "type": "string" }
    }
  },
  "capture": {
    "token": "token"
  }
}
"#;
const GITIGNORE: &str = "# 运行历史（CLI / Desktop 共享，本地状态不入库）\n.codebaby/\n# 本机覆盖（密钥 / 端口，不入库）\nenvs/*.local.json\n";

const LOCAL_IGNORE: &str = "envs/*.local.json";

/// 项目根 .gitignore 是否已忽略本机覆盖文件
pub fn local_env_ignored(root: &Path) -> bool {
    std::fs::read_to_string(root.join(".gitignore"))
        .is_ok_and(|t| t.lines().any(|l| matches!(l.trim(), "envs/*.local.json" | "*.local.json" | "/envs/*.local.json")))
}

/// 写本机覆盖文件前调用：确保项目根 .gitignore 忽略 `envs/*.local.json`（没有则追加 / 新建）
pub fn ensure_local_env_ignored(root: &Path) -> Result<()> {
    if local_env_ignored(root) {
        return Ok(());
    }
    let path = root.join(".gitignore");
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str("# 本机覆盖（密钥 / 端口，不入库）\n");
    text.push_str(LOCAL_IGNORE);
    text.push('\n');
    crate::fsx::write(&path, text).with_context(|| format!("写入失败: {}", path.display()))
}

/// 初始化项目脚手架（已存在的文件跳过，不覆盖）
pub fn init_project(dir: &Path) -> Result<Vec<ScaffoldEntry>> {
    init_project_with(dir, None)
}

/// 初始化项目；`base_url` 给定就写成 dev 环境的被测服务地址（缺省 http://localhost:8080）
pub fn init_project_with(dir: &Path, base_url: Option<&str>) -> Result<Vec<ScaffoldEntry>> {
    use crate::runner::discover;
    let env_dev = match base_url.map(str::trim).filter(|u| !u.is_empty()) {
        Some(u) => {
            if !(u.starts_with("http://") || u.starts_with("https://")) {
                anyhow::bail!("--base-url 要以 http:// 或 https:// 开头：{u}");
            }
            serde_json::to_string_pretty(&serde_json::json!({ "vars": { "baseUrl": u.trim_end_matches('/') } }))? + "\n"
        }
        None => ENV_DEV.to_string(),
    };
    let files: [(&str, &str); 7] = [
        (discover::PROJECT_FILE, PROJECT),
        ("envs/dev.json", &env_dev),
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
    out.extend(crate::guide::write_agent_entry(dir)?);
    Ok(out)
}

fn write_new(path: &Path, content: &str) -> Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("创建目录失败: {}", parent.display()))?;
    }
    crate::fsx::write(path, content).with_context(|| format!("写入失败: {}", path.display()))?;
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
    if matches!(n, crate::runner::discover::PROJECT_FILE | "collection.json" | "envs") {
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
/// 在 dir 下新建用例时的文件名：`NN-名称.json`（NN 接在已有编号之后）。Desktop 新建对话框据此预览
pub fn case_file_name(dir: &Path, name: &str) -> String {
    format!("{:02}-{}.json", next_prefix(dir), slug(name))
}

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
    let file = dir.join(case_file_name(dir, name));
    let doc = serde_json::json!({
        "name": name.trim(),
        "request": {
            "method": method.to_uppercase(),
            "url": if url.is_empty() { "{{baseUrl}}/" } else { url },
        },
        "expect": { "status": 200 },
    });
    if !write_new(&file, &crate::fmt::canonical_case_text(&doc))? {
        bail!("文件已存在: {}", file.display());
    }
    Ok(file)
}

/// 新建 gRPC 用例：message 缺省用方法的请求骨架（调用方从 .proto / 反射取到时传入）
pub fn new_grpc_case(dir: &Path, name: &str, target: &str, method: &str, message: Option<serde_json::Value>) -> Result<PathBuf> {
    let doc = serde_json::json!({
        "name": name.trim(),
        "grpc": {
            "target": if target.trim().is_empty() { "{{grpcTarget}}" } else { target.trim() },
            "method": method.trim(),
            "message": message.unwrap_or_else(|| serde_json::json!({})),
        },
        "expect": { "status": "OK" },
    });
    write_case(dir, name, &doc)
}

/// 新建 WebSocket 用例：连上即算通过（101），对话步骤在 ws.steps 里补
pub fn new_ws_case(dir: &Path, name: &str, url: &str) -> Result<PathBuf> {
    let doc = serde_json::json!({
        "name": name.trim(),
        "ws": {
            "url": if url.trim().is_empty() { "{{wsBase}}/" } else { url.trim() },
            "steps": [],
        },
        "expect": { "status": 101 },
    });
    write_case(dir, name, &doc)
}

/// 新建 Web 用例：打开页面、页面返回 200 即通过（自动留下打开后的截图），操作步骤在 web.steps 里补
pub fn new_web_case(dir: &Path, name: &str, url: &str) -> Result<PathBuf> {
    let doc = serde_json::json!({
        "name": name.trim(),
        "web": {
            "url": if url.trim().is_empty() { "{{webBase}}/" } else { url.trim() },
            "steps": [],
        },
        "expect": { "status": 200 },
    });
    write_case(dir, name, &doc)
}

/// 新建 Android 用例：启动应用即通过（自动留下启动后的截图），操作步骤在 android.steps 里补
pub fn new_android_case(dir: &Path, name: &str, app: &str) -> Result<PathBuf> {
    if app.trim().is_empty() {
        bail!("写应用包名，如 com.example.shop");
    }
    let doc = serde_json::json!({
        "name": name.trim(),
        "android": { "app": app.trim(), "steps": [] },
    });
    write_case(dir, name, &doc)
}

/// 新建 Mac 桌面用例：启动（或接管）应用即通过（自动留下启动后的截图），操作步骤在 desktop.steps 里补
pub fn new_desktop_case(dir: &Path, name: &str, app: &str) -> Result<PathBuf> {
    if app.trim().is_empty() {
        bail!("写应用名称（如 TextEdit）或 bundle ID（如 com.apple.TextEdit）");
    }
    let doc = serde_json::json!({
        "name": name.trim(),
        "desktop": { "app": app.trim(), "steps": [] },
    });
    write_case(dir, name, &doc)
}

fn write_case(dir: &Path, name: &str, doc: &serde_json::Value) -> Result<PathBuf> {
    if name.trim().is_empty() {
        bail!("用例名不能为空");
    }
    std::fs::create_dir_all(dir)?;
    let file = dir.join(case_file_name(dir, name));
    if !write_new(&file, &crate::fmt::canonical_case_text(doc))? {
        bail!("文件已存在: {}", file.display());
    }
    Ok(file)
}

/// 从 curl 命令新建用例（Desktop「从 cURL 新建」与 CLI `new case --curl` 共用）：
/// - `base_url` 给定且 URL 以它开头 → 替换为 `{{baseUrl}}`，用例跨环境可用
/// - 名称缺省取「方法 + 路径末段」
/// - multipart 的 @文件 相对 `cwd` 存在时写成绝对路径（粘贴来源的工作目录与用例目录不同）
pub fn new_case_from_curl(dir: &Path, name: Option<&str>, curl: &str, base_url: Option<&str>, cwd: Option<&Path>) -> Result<PathBuf> {
    let mut doc = crate::importer::import_curl(curl)?;
    let url = doc["request"]["url"].as_str().unwrap_or_default().to_string();
    if let Some(base) = base_url.map(|b| b.trim_end_matches('/')).filter(|b| !b.is_empty()) {
        if let Some(rest) = url.strip_prefix(base).filter(|r| r.is_empty() || r.starts_with(['/', '?'])) {
            doc["request"]["url"] = serde_json::json!(format!("{{{{baseUrl}}}}{rest}"));
        }
    }
    if let (Some(cwd), Some(parts)) = (cwd, doc.pointer_mut("/request/body/multipart").and_then(|m| m.as_object_mut())) {
        for v in parts.values_mut() {
            if let Some(f) = v.get("file").and_then(|f| f.as_str()).map(PathBuf::from) {
                if f.is_relative() && cwd.join(&f).is_file() {
                    v["file"] = serde_json::json!(cwd.join(&f).to_string_lossy());
                }
            }
        }
    }
    let method = doc["request"]["method"].as_str().unwrap_or("GET").to_string();
    let name = match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => n.to_string(),
        None => {
            let path = url.split(['?', '#']).next().unwrap_or("").split("://").last().unwrap_or("");
            let seg = path.split('/').skip(1).filter(|s| !s.is_empty() && !s.contains("{{")).last().unwrap_or("request");
            format!("{} {seg}", method.to_lowercase())
        }
    };
    doc["name"] = serde_json::json!(name);
    std::fs::create_dir_all(dir)?;
    let file = dir.join(case_file_name(dir, &name));
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

/// 复制用例文件 / 集合目录到同级的新名称（用例自动补 .json）；用例与集合的 `name` 改为新名称，避免两份同名
pub fn duplicate(path: &Path, new_name: &str) -> Result<PathBuf> {
    check_name(new_name)?;
    let parent = path.parent().ok_or_else(|| anyhow!("无法复制根目录"))?;
    let is_case = path.is_file();
    let mut name = new_name.trim().to_string();
    if is_case && !name.ends_with(".json") {
        name.push_str(".json");
    }
    let target = parent.join(&name);
    if target.exists() {
        bail!("已存在同名文件: {name}");
    }
    if is_case {
        std::fs::copy(path, &target).with_context(|| format!("复制失败: {}", path.display()))?;
        retitle(&target, name.trim_end_matches(".json"), crate::fmt::canonical_case_text)?;
    } else {
        copy_dir(path, &target)?;
        let cfg = target.join("collection.json");
        if cfg.exists() {
            retitle(&cfg, &name, crate::fmt::canonical_collection_text)?;
        }
    }
    Ok(target)
}

/// 已有 name 字段的文件：改为新名称（用例去掉 NN- 编号前缀）
fn retitle(file: &Path, name: &str, canonical: fn(&serde_json::Value) -> String) -> Result<()> {
    let mut doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(file)?).with_context(|| format!("解析失败: {}", file.display()))?;
    if let Some(obj) = doc.as_object_mut().filter(|o| o.contains_key("name")) {
        let title = name.split_once('-').filter(|(n, _)| n.chars().all(|c| c.is_ascii_digit())).map_or(name, |(_, t)| t);
        obj.insert("name".into(), title.into());
        crate::fsx::write(file, canonical(&doc))?;
    }
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)?.flatten() {
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), &dest).with_context(|| format!("复制失败: {}", entry.path().display()))?;
        }
    }
    Ok(())
}

/// 删除结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deleted {
    /// 已移到系统废纸篓（可还原）
    Trashed,
    /// 系统不支持废纸篓（如无桌面环境的 Linux），已永久删除
    Removed,
}

/// 删除用例文件 / 集合目录 / 环境文件：优先移到系统废纸篓，不支持时才永久删除
pub fn delete(path: &Path) -> Result<Deleted> {
    if !path.exists() {
        bail!("不存在: {}", path.display());
    }
    if trash::delete(path).is_ok() && !path.exists() {
        return Ok(Deleted::Trashed);
    }
    if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
    .with_context(|| format!("删除失败: {}", path.display()))?;
    Ok(Deleted::Removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DevTools「Copy as cURL」原样粘贴：续行、$'…'、-b、--data-raw JSON
    #[test]
    fn new_case_from_browser_curl() {
        let dir = tempfile::tempdir().unwrap();
        let cmd = "curl 'https://api.x.com/v1/orders?page=2' \\\n  -H 'accept: application/json' \\\n  -H 'content-type: application/json' \\\n  -b 'sid=abc; theme=dark' \\\n  --data-raw $'{\"note\":\"it\\'s\"}'";
        let f = new_case_from_curl(dir.path(), None, cmd, Some("https://api.x.com/"), None).unwrap();
        assert_eq!(f.file_name().unwrap(), "01-post-orders.json");
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(v["request"]["method"], "POST");
        assert_eq!(v["request"]["url"], "{{baseUrl}}/v1/orders?page=2");
        assert_eq!(v["request"]["body"]["json"]["note"], "it's");
        assert_eq!(v["request"]["headers"]["Cookie"], "sid=abc; theme=dark");
        assert!(v["request"]["headers"].get("content-type").is_none(), "json body 自动补 Content-Type");
        crate::config::load_strict::<crate::config::TestCase>(&f).unwrap();
    }

    #[test]
    fn curl_form_multipart_get_and_auth() {
        let v = crate::importer::import_curl("curl -u admin:pw -F title=报告 -F 'f=@./a.png;type=image/png' http://h/up").unwrap();
        assert_eq!(v["request"]["method"], "POST");
        assert_eq!(v["request"]["headers"]["Authorization"], "Basic YWRtaW46cHc=");
        assert_eq!(v["request"]["body"]["multipart"]["title"], "报告");
        assert_eq!(v["request"]["body"]["multipart"]["f"]["file"], "./a.png");
        assert_eq!(v["request"]["body"]["multipart"]["f"]["type"], "image/png");
        let v = crate::importer::import_curl("curl -G http://h/s -d q=1 --data-urlencode 'w=a b'").unwrap();
        assert_eq!(v["request"]["method"], "GET");
        assert_eq!(v["request"]["query"]["q"], "1");
        assert_eq!(v["request"]["query"]["w"], "a b");
        let v = crate::importer::import_curl("curl -X PUT --data-urlencode 'w=a b' --url=http://h/s").unwrap();
        assert_eq!(v["request"]["method"], "PUT");
        assert_eq!(v["request"]["body"]["form"]["w"], "a b");
        // 自己的 dry-run 输出能原样导回
        let v = crate::importer::import_curl(r#"curl -sS --max-time 30 -X POST 'http://x/api?a=1&b=2' -H 'Content-Type: application/json' --data-raw '{"name":"O'\''Brien"}'"#).unwrap();
        assert_eq!(v["request"]["body"]["json"]["name"], "O'Brien");
        assert_eq!(v["request"]["timeoutMs"], 30000);
    }

    #[test]
    fn new_case_uses_next_prefix_and_canonical_form() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("01-a.json"), "{}").unwrap();
        std::fs::write(dir.path().join("07-b.json"), "{}").unwrap();
        let f = new_case(dir.path(), "Create Order", "post", "").unwrap();
        assert_eq!(f.file_name().unwrap(), "08-create-order.json");
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(v["request"]["method"], "POST");
        assert_eq!(v["expect"], serde_json::json!({"status": 200}), "新用例默认断言状态码（声明式 expect）");
        assert!(v.get("tests").is_none());
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

    #[test]
    fn duplicate_case_and_collection_retitles() {
        let dir = tempfile::tempdir().unwrap();
        init_project(dir.path()).unwrap();
        let demo = dir.path().join("collections/demo");
        let copy = duplicate(&demo.join("01-health.json"), "03-health-v2").unwrap();
        assert_eq!(copy.file_name().unwrap(), "03-health-v2.json");
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&copy).unwrap()).unwrap();
        assert_eq!(v["name"], "health-v2");
        assert!(duplicate(&demo.join("01-health.json"), "03-health-v2").unwrap_err().to_string().contains("已存在"));
        let col = duplicate(&demo, "demo-copy").unwrap();
        assert!(col.join("01-health.json").exists() && col.join("03-health-v2.json").exists());
        let c: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(col.join("collection.json")).unwrap()).unwrap();
        assert_eq!(c["name"], "demo-copy");
    }
}
