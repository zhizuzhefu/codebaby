//! 使用文档：编译进二进制，内容与所装版本严格一致，脱离本仓库也能获取。三层：
//!
//! 1. 速查卡（docs/guide/quickstart.md）：`requestbaby guide` 默认输出，读完就能写出能通过校验的用例
//! 2. 场景配方 / 主题（docs/guide/<主题>.md）：`requestbaby guide <主题>`，validate 的提示里带主题名
//! 3. 字段参考：`requestbaby guide ref`，由内置 JSON Schema 生成（与校验用的是同一份，不会过期）
//!
//! Desktop「复制 AI 对接指南」复制速查卡；[`write_agent_entry`] 写项目内的 AGENTS.md / CLAUDE.md 入口。
use crate::scaffold::ScaffoldEntry;
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

/// 速查卡
pub const QUICKSTART: &str = include_str!("../../../docs/guide/quickstart.md");

/// 主题：(名称, 中文别名, 标题, 正文)
pub const TOPICS: &[(&str, &[&str], &str, &str)] = &[
    ("expect", &["断言"], "断言全集", include_str!("../../../docs/guide/expect.md")),
    ("capture", &["捕获", "串联"], "捕获与串联", include_str!("../../../docs/guide/capture.md")),
    ("vars", &["变量"], "变量进阶", include_str!("../../../docs/guide/vars.md")),
    ("flow", &["流程", "workflow"], "流程", include_str!("../../../docs/guide/flow.md")),
    ("auth", &["鉴权", "登录"], "配方：鉴权登录", include_str!("../../../docs/guide/auth.md")),
    ("e2e", &["链路"], "配方：多步链路", include_str!("../../../docs/guide/e2e.md")),
    ("data", &["数据驱动", "数据"], "配方：数据驱动", include_str!("../../../docs/guide/data.md")),
    ("setup", &["造数", "清数"], "配方：造数清数", include_str!("../../../docs/guide/setup.md")),
    ("env", &["环境", "固定环境"], "环境与固定环境", include_str!("../../../docs/guide/env.md")),
    ("grpc", &["rpc"], "gRPC", include_str!("../../../docs/guide/grpc.md")),
    ("ws", &["websocket", "长连接"], "WebSocket", include_str!("../../../docs/guide/ws.md")),
    ("web", &["界面", "浏览器", "页面"], "Web 界面验收", include_str!("../../../docs/guide/web.md")),
    ("android", &["安卓", "app", "手机"], "Android 界面验收", include_str!("../../../docs/guide/android.md")),
    ("desktop", &["mac", "桌面", "macos", "agent-desktop"], "Mac 桌面应用验收", include_str!("../../../docs/guide/desktop.md")),
    ("ocr", &["文字识别", "屏幕文字", "识别"], "屏幕文字识别（OCR）", include_str!("../../../docs/guide/ocr.md")),
    ("control", &["循环", "监控", "回调", "条件", "随机等待", "repeat", "exec"], "循环、条件、随机等待与回调", include_str!("../../../docs/guide/control.md")),
    ("proxy", &["代理", "socks", "socks5"], "代理", include_str!("../../../docs/guide/proxy.md")),
    ("scripts", &["脚本", "js"], "进阶 JS 脚本", include_str!("../../../docs/guide/scripts.md")),
    ("cli", &["命令", "协议"], "命令与调度协议", include_str!("../../../docs/guide/cli.md")),
];

fn header() -> String {
    format!("<!-- requestbaby {} · `requestbaby guide` 输出，与已安装版本一致 -->\n\n", env!("CARGO_PKG_VERSION"))
}

/// 默认输出：速查卡
pub fn quickstart() -> String {
    format!("{}{}", header(), QUICKSTART.trim_end())
}

/// 按名称（或中文别名）取主题；`ref` = 字段参考，`all` = 全部
pub fn topic(name: &str) -> Option<String> {
    let n = name.trim().to_lowercase();
    match n.as_str() {
        "" | "quickstart" | "速查" | "速查卡" => return Some(quickstart()),
        "ref" | "reference" | "字段" | "字段参考" => return Some(format!("{}{}", header(), reference())),
        "all" | "全部" => return Some(full()),
        _ => {}
    }
    TOPICS
        .iter()
        .find(|(key, alias, _, _)| *key == n || alias.iter().any(|a| a.to_lowercase() == n))
        .map(|(_, _, _, text)| format!("{}{}", header(), text.trim_end()))
}

/// 主题清单（未知主题时提示）
pub fn topic_list() -> String {
    let mut lines: Vec<String> = TOPICS.iter().map(|(k, alias, title, _)| format!("  {k:<8} {title}（{}）", alias.join(" / "))).collect();
    lines.push(format!("  {:<8} 全部字段参考（由 JSON Schema 生成）", "ref"));
    lines.push(format!("  {:<8} 速查卡 + 全部主题 + 字段参考", "all"));
    lines.join("\n")
}

/// 全部：速查卡 + 各主题 + 字段参考
pub fn full() -> String {
    let mut out = vec![quickstart()];
    out.extend(TOPICS.iter().map(|(_, _, _, text)| text.trim_end().to_string()));
    out.push(reference());
    out.join("\n\n---\n\n")
}

// ---------------------------------------------------------------- 字段参考（由 schema 生成）

/// 字段参考：四类配置文件的全部字段（类型 + 说明），以及 expect 的运算符
pub fn reference() -> String {
    let mut out = String::from("# 字段参考\n\n> `requestbaby guide ref` · 由内置 JSON Schema 生成，与 validate 使用同一份；完整 Schema：`requestbaby schema --all`\n");
    for (file, schema) in [
        ("用例 collections/**/NN-名称.json", crate::validate::CASE_SCHEMA),
        ("集合 collection.json", crate::validate::COLLECTION_SCHEMA),
        ("环境 envs/<环境>.json", crate::validate::ENV_SCHEMA),
        ("项目 requestbaby.json", crate::validate::PROJECT_SCHEMA),
    ] {
        let root: Value = serde_json::from_str(schema).expect("内置 schema");
        out.push_str(&format!("\n## {file}\n\n| 字段 | 类型 | 说明 |\n|------|------|------|\n"));
        for (name, ty, desc) in schema_rows(&root) {
            out.push_str(&format!("| `{name}` | {ty} | {} |\n", desc.replace('|', "\\|").replace('\n', " ")));
        }
    }
    let case: Value = serde_json::from_str(crate::validate::CASE_SCHEMA).expect("内置 schema");
    if let Some(ops) = case.pointer("/definitions/matcher/anyOf/1/properties").and_then(Value::as_object) {
        out.push_str("\n## expect 运算符\n\n| 运算符 | 说明 |\n|--------|------|\n");
        for (op, def) in ops {
            out.push_str(&format!("| `{op}` | {} |\n", def.get("description").and_then(Value::as_str).unwrap_or_default()));
        }
    }
    out
}

/// schema → (字段路径, 类型, 说明)：展开 properties / $ref / 命名对象（additionalProperties）/ 数组元素
pub fn schema_rows(root: &Value) -> Vec<(String, String, String)> {
    let mut rows = vec![];
    walk(root, root, "", &mut rows, &mut vec![]);
    rows
}

fn resolve<'a>(root: &'a Value, v: &'a Value) -> &'a Value {
    match v.get("$ref").and_then(Value::as_str).and_then(|r| r.strip_prefix('#')) {
        Some(ptr) => root.pointer(ptr).unwrap_or(v),
        None => v,
    }
}

/// seen：正在展开的 $ref。步骤可以嵌套自己（repeat / if 里还是步骤），遇到已在展开的引用只列这一行、不再往里走
fn walk(root: &Value, node: &Value, prefix: &str, rows: &mut Vec<(String, String, String)>, seen: &mut Vec<String>) {
    let node = resolve(root, node);
    let Some(props) = node.get("properties").and_then(Value::as_object) else { return };
    let ref_of = |v: &Value| v.get("$ref").and_then(Value::as_str).map(str::to_string);
    for (key, raw) in props {
        let def = resolve(root, raw);
        let name = format!("{prefix}{key}");
        let desc = raw.get("description").or_else(|| def.get("description")).and_then(Value::as_str).unwrap_or_default().to_string();
        rows.push((name.clone(), type_of(root, def), desc));
        let mut descend = |r: Option<String>, target: &Value, path: String, rows: &mut Vec<(String, String, String)>| {
            if let Some(r) = &r {
                if seen.contains(r) {
                    return;
                }
                seen.push(r.clone());
            }
            walk(root, target, &path, rows, seen);
            if r.is_some() {
                seen.pop();
            }
        };
        if def.get("properties").is_some() {
            descend(ref_of(raw), def, format!("{name}."), rows);
        }
        // 命名对象：workflows.<名称>.steps
        if let Some(ap_raw) = def.get("additionalProperties").filter(|a| resolve(root, a).get("properties").is_some()) {
            descend(ref_of(ap_raw), resolve(root, ap_raw), format!("{name}.<名称>."), rows);
        }
        if let Some(items_raw) = def.get("items").filter(|i| resolve(root, i).get("properties").is_some()) {
            descend(ref_of(items_raw), resolve(root, items_raw), format!("{name}[]."), rows);
        }
    }
}

fn type_of(root: &Value, def: &Value) -> String {
    let def = resolve(root, def);
    if let Some(e) = def.get("enum").and_then(Value::as_array) {
        return e.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" / ");
    }
    let one = |d: &Value| -> Option<String> {
        match d.get("type")? {
            Value::String(t) => Some(match (t.as_str(), d.get("items").and_then(|i| i.get("type")).and_then(Value::as_str)) {
                ("array", Some(it)) => format!("{it}[]"),
                (t, _) => t.to_string(),
            }),
            Value::Array(ts) => Some(ts.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" / ")),
            _ => None,
        }
    };
    if let Some(t) = one(def) {
        return t;
    }
    for key in ["oneOf", "anyOf"] {
        if let Some(alts) = def.get(key).and_then(Value::as_array) {
            let mut ts: Vec<String> = alts.iter().filter_map(|a| one(resolve(root, a))).collect();
            ts.dedup();
            if !ts.is_empty() {
                return ts.join(" / ");
            }
        }
    }
    "任意".into()
}

// ---------------------------------------------------------------- 项目内 AI 入口

/// 项目内的 AI 入口：通用约定 AGENTS.md（Codex / Cursor 等读取）
pub const AGENTS_MD: &str = r#"# requestbaby 接口测试项目

本目录用 [requestbaby](https://github.com/zhizuzhefu/requestbaby) 做接口自动化测试。AI 编码助手在编写或修改这里的用例前，先执行：

```bash
requestbaby guide          # 速查卡：5 个概念、典型用例、expect / capture 写法、闭环命令（与已安装版本一致，必读）
requestbaby guide <主题>   # 按场景深入：expect capture vars flow auth e2e data setup env web android desktop scripts cli ref
```

闭环：写用例 → `requestbaby validate --report json`（exit 2 = 配置错误；warn 是写法建议，`requestbaby fmt --fix` 自动改写）
→ `requestbaby run --report json`（exit 1 = 断言失败，读 results[].tests[].detail 修；`requestbaby run --failed` 只重跑失败的）
→ `requestbaby verify --plan` 预览 → `requestbaby verify --report json`（核对 coverage，exit 0 才算完成）。明确验收流程时用 `--workflow`，约定的必验文件用 `--require-case` 固定（guide cli）。

约定：
- 断言写 `expect`，取值写 `capture`；只有跨字段计算才用 JS（tests / post）
- 用例 = `collections/**/NN-名称.json`，文件名序即执行序；新建用 `requestbaby new case <集合目录> <名称> -m <方法>`，已有 curl 用 `--curl "curl …"`
- 期望结果从需求或契约确定；`requestbaby send <用例> --suggest` 用于查看响应路径和断言写法
- URL 写 `{{baseUrl}}/路径`；变量用 `requestbaby vars set …` 写进环境，密钥加 `--secret`，本机真实值用 `--local <环境>`
- 只能在某个环境跑的，用流程固定环境（`requestbaby workflow env <流程> <环境>`），不写环境守卫脚本
- 界面用例（web / android / desktop）：先 `requestbaby web snapshot <url>` / `android snapshot` / `desktop snapshot <应用>` 看元素与定位；API、Web、Android、Mac 桌面可放进同一个流程，`capture` 的值互通
- 每个场景都要先登录的：登录做成一个流程，其他流程 `needs` 依赖它，不要重复登录；切用户用步骤的 `as`（`requestbaby guide flow`）
- 被测服务由你自己启动；`.requestbaby/` 是运行历史，不要修改
"#;

/// Claude Code 读取 CLAUDE.md：只引用 AGENTS.md，内容只维护一份
pub const CLAUDE_MD: &str = "@AGENTS.md\n";

/// 项目里 AGENTS.md 的状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentEntryStatus {
    Missing,
    /// 与已安装版本一致
    Current,
    /// 旧版 requestbaby 生成的入口（标题相同、内容不同）：可用 `guide --init --update` 更新
    Outdated,
    /// 用户自己维护的文件：不检查、不覆盖
    Custom,
}

const AGENTS_TITLE: &str = "# requestbaby 接口测试项目";

pub fn agent_entry_status(root: &Path) -> AgentEntryStatus {
    match std::fs::read_to_string(root.join("AGENTS.md")) {
        Err(_) => AgentEntryStatus::Missing,
        Ok(t) if t == AGENTS_MD => AgentEntryStatus::Current,
        Ok(t) if t.starts_with(AGENTS_TITLE) => AgentEntryStatus::Outdated,
        Ok(_) => AgentEntryStatus::Custom,
    }
}

/// 在项目根写入 AI 入口文件；已存在的跳过（用户可能已有自己的 AGENTS.md / CLAUDE.md）
pub fn write_agent_entry(root: &Path) -> Result<Vec<ScaffoldEntry>> {
    write_agent_entry_with(root, false)
}

/// update = true：旧版 requestbaby 生成的 AGENTS.md 更新为当前版本（用户自己写的文件仍不覆盖）
pub fn write_agent_entry_with(root: &Path, update: bool) -> Result<Vec<ScaffoldEntry>> {
    let mut out = vec![];
    for (name, content) in [("AGENTS.md", AGENTS_MD), ("CLAUDE.md", CLAUDE_MD)] {
        let path = root.join(name);
        let stale = update && name == "AGENTS.md" && agent_entry_status(root) == AgentEntryStatus::Outdated;
        let created = !path.exists() || stale;
        if created {
            crate::fsx::write(&path, content).with_context(|| format!("写入失败: {}", path.display()))?;
        }
        out.push(ScaffoldEntry { path, created });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quickstart_is_small_and_topics_resolve() {
        assert!(QUICKSTART.len() <= 6 * 1024, "速查卡应保持在 6KB 内：{} 字节", QUICKSTART.len());
        for (key, alias, _, _) in TOPICS {
            assert!(topic(key).is_some(), "{key}");
            for a in *alias {
                assert_eq!(topic(a), topic(key), "{a}");
            }
            assert!(QUICKSTART.contains(&format!("`{key}`")), "速查卡末尾要列出主题 {key}");
        }
        assert!(topic("ref").unwrap().contains("| `expect.body` |"));
        assert!(topic("nope").is_none());
        let all = full();
        assert!(all.contains("# requestbaby 速查卡") && all.contains("# 字段参考") && all.contains("# 配方：数据驱动"));
    }

    /// 字段参考由 schema 生成：每个字段都必须有说明，否则参考表里会出现空格子
    #[test]
    fn every_schema_field_is_described() {
        for schema in [crate::validate::CASE_SCHEMA, crate::validate::COLLECTION_SCHEMA, crate::validate::ENV_SCHEMA, crate::validate::PROJECT_SCHEMA] {
            let root: Value = serde_json::from_str(schema).unwrap();
            for (name, ty, desc) in schema_rows(&root) {
                assert!(!desc.trim().is_empty(), "{} 的字段 {name}（{ty}）缺少 description", root["title"]);
            }
        }
        let r = reference();
        for row in ["| `request.method` | GET / POST", "| `expect.status` |", "| `workflows.<名称>.steps[].run` |", "| `capture` | object |"] {
            assert!(r.contains(row), "{row}\n{r}");
        }
    }

    /// 校验 / 改写提示里写的「guide <主题>」必须是真实主题（提示即入口）
    #[test]
    fn hints_point_to_real_topics() {
        let sources = [include_str!("teach.rs"), include_str!("validate.rs"), include_str!("expect.rs"), include_str!("../../requestbaby-cli/src/main.rs")];
        let mut seen = 0;
        for src in sources {
            for part in src.split("guide ").skip(1) {
                let word: String = part.chars().take_while(|c| c.is_ascii_lowercase() || *c == '2').collect();
                if word.is_empty() || matches!(word.as_str(), "all" | "ref") {
                    continue;
                }
                assert!(topic(&word).is_some(), "提示里引用了不存在的主题 guide {word}");
                seen += 1;
            }
        }
        assert!(seen >= 5, "{seen}");
    }

    #[test]
    fn outdated_agent_entry_is_detected_and_updated_only_on_request() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(agent_entry_status(root), AgentEntryStatus::Missing);
        write_agent_entry(root).unwrap();
        assert_eq!(agent_entry_status(root), AgentEntryStatus::Current);
        std::fs::write(root.join("AGENTS.md"), format!("{AGENTS_TITLE}\n\n旧版内容\n")).unwrap();
        assert_eq!(agent_entry_status(root), AgentEntryStatus::Outdated);
        write_agent_entry(root).unwrap();
        assert_eq!(agent_entry_status(root), AgentEntryStatus::Outdated, "不带 update 不覆盖");
        let e = write_agent_entry_with(root, true).unwrap();
        assert!(e[0].created && !e[1].created);
        assert_eq!(agent_entry_status(root), AgentEntryStatus::Current);
        std::fs::write(root.join("AGENTS.md"), "# 我们团队的约定\n").unwrap();
        assert_eq!(agent_entry_status(root), AgentEntryStatus::Custom);
        write_agent_entry_with(root, true).unwrap();
        assert_eq!(std::fs::read_to_string(root.join("AGENTS.md")).unwrap(), "# 我们团队的约定\n", "用户自己的文件永不覆盖");
    }

    #[test]
    fn agent_entry_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("CLAUDE.md"), "mine\n").unwrap();
        let entries = write_agent_entry(dir.path()).unwrap();
        assert!(entries[0].created && !entries[1].created);
        assert_eq!(std::fs::read_to_string(dir.path().join("CLAUDE.md")).unwrap(), "mine\n");
        assert!(std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap().contains("requestbaby guide"));
    }
}
