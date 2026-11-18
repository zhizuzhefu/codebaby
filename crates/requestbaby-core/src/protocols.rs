//! 各协议的项目级操作（CLI 与 Desktop 用例编辑器共用）：
//! - gRPC：`grpc list / describe`，接口描述来源按 requestbaby.json 的 grpc 配置
//! - Web / Android：`web snapshot` / `android snapshot` 列出界面元素与可直接粘贴的定位
//!
//! 地址里的 {{变量}} 按所选环境替换
use crate::engine::grpc::{self, MethodInfo, Source};
use anyhow::Result;
use std::path::Path;

/// 按环境替换地址里的变量（项目变量 + 环境变量 + 本机覆盖）
pub fn render(root: &Path, env: &str, raw: &str) -> Result<String> {
    let ctx = crate::runner::load_project_ctx(root, env)?;
    let mut v = crate::vars::Vars::new();
    v.push_str_layer(ctx.project.vars.plain());
    v.push_str_layer(ctx.env_vars.clone());
    let out = v.render(raw);
    if let Some(lo) = crate::runner::scan_leftover_placeholders(&out).first() {
        anyhow::bail!("{raw} 里的变量 {lo} 在环境 {env} 中没有定义（requestbaby vars set {lo} <值> -e {env}）");
    }
    Ok(out)
}

/// 项目的接口描述来源（配置了 grpc.protos 用 .proto，否则服务端反射）
pub fn grpc_source(root: &Path) -> Result<Source> {
    Source::from_config(root, &project(root).grpc)
}

/// 项目在该环境下的代理规则（与运行用例时一致）
pub fn proxy(root: &Path, env: &str) -> Result<crate::engine::proxy::ProxyPolicy> {
    Ok(crate::runner::load_project_ctx(root, env)?.proxy)
}

/// 不分环境的操作（设备列表 / 界面快照）用缺省环境的代理规则；不在项目里时只看环境变量
fn default_proxy(root: Option<&Path>) -> Result<crate::engine::proxy::ProxyPolicy> {
    match root {
        Some(r) => proxy(r, &crate::session::resolve_env(&[r.to_path_buf()], None)),
        None => crate::engine::proxy::ProxyPolicy::resolve(None, |s| s.to_string()),
    }
}

fn project(root: &Path) -> crate::config::Project {
    crate::runner::discover::project_file(root).and_then(|p| crate::config::load_strict(&p).ok()).unwrap_or_default()
}

/// 查询接口最多等 15 秒（编辑器里点「加载方法」不该卡太久）
fn timeout(root: &Path) -> u64 {
    project(root).timeout_ms.min(15_000)
}

/// 列出服务的全部方法
pub fn grpc_methods(root: &Path, env: &str, target: &str, insecure: bool) -> Result<Vec<MethodInfo>> {
    let source = grpc_source(root)?;
    let target = if matches!(source, Source::Reflection) { render(root, env, target)? } else { String::new() };
    grpc::list_methods(&target, insecure, timeout(root), &source, &proxy(root, env)?)
}

/// 方法详情与请求骨架
pub fn grpc_describe(root: &Path, env: &str, target: &str, method: &str, insecure: bool) -> Result<MethodInfo> {
    let source = grpc_source(root)?;
    let target = if matches!(source, Source::Reflection) { render(root, env, target)? } else { String::new() };
    grpc::describe_method(&target, method, insecure, timeout(root), &source, &proxy(root, env)?)
}

// ---------------------------------------------------------------------------------------------
// 界面快照：列出页面 / 界面上的元素和可直接粘贴的定位（CLI `web snapshot` / `android snapshot`、Desktop 步骤编辑器共用）
// ---------------------------------------------------------------------------------------------

/// 快照里的一个元素
#[derive(Debug, Clone, serde::Serialize)]
pub struct SnapElement {
    /// 中文角色名：按钮 / 链接 / 输入框…
    pub kind: String,
    pub name: String,
    /// 可直接粘贴进步骤的定位
    pub locator: serde_json::Value,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub states: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct UiSnapshot {
    /// Web：地址；Android：包名
    pub location: String,
    /// Web：标题；Android：Activity
    pub title: String,
    pub elements: Vec<SnapElement>,
    /// 截图（PNG）
    pub screenshot: Option<std::path::PathBuf>,
}

fn snapshot_dir(root: Option<&Path>) -> std::path::PathBuf {
    match root {
        Some(r) => crate::history::scratch_dir(r),
        None => std::env::temp_dir().join("requestbaby-evidence"),
    }
}

/// 打开页面，列出可操作的元素与推荐定位（角色 + 名称；同名多个时加 nth；输入框用标签）
pub fn web_snapshot(root: Option<&Path>, env: &str, url: &str, headed: bool) -> Result<UiSnapshot> {
    use crate::engine::ui::UiDriver;
    let project = root.map(project).unwrap_or_default();
    let url = match root {
        Some(r) if url.contains("{{") => render(r, env, url)?,
        _ => url.to_string(),
    };
    let proxy = match root {
        Some(r) => proxy(r, env)?,
        None => default_proxy(None)?,
    };
    let mut b = crate::engine::cdp::open_browser(&project.web, &proxy, headed || project.web.headed.unwrap_or(false), std::time::Duration::from_secs(30))?;
    let vp = project.web.viewport.unwrap_or_default();
    let mut page = crate::engine::web::WebPage::open(&mut b, (vp.width, vp.height), std::time::Duration::from_millis(project.timeout_ms.max(1000)))?;
    page.goto(&url)?;
    page.settle(std::time::Duration::from_secs(5));
    let info = page.page()?;
    let raw = page.ax_elements()?;
    let mut seen: std::collections::HashMap<(String, String), usize> = Default::default();
    let totals = raw.iter().fold(std::collections::HashMap::<(String, String), usize>::new(), |mut m, (r, n, _)| {
        *m.entry((r.clone(), n.clone())).or_default() += 1;
        m
    });
    let form = ["textbox", "searchbox", "combobox", "spinbutton"];
    let elements = raw
        .into_iter()
        .map(|(role, name, states)| {
            let key = (role.clone(), name.clone());
            let i = *seen.entry(key.clone()).and_modify(|x| *x += 1).or_insert(0);
            let many = totals.get(&key).copied().unwrap_or(1) > 1;
            let role_key = if role == "PopUpButton" { "combobox".to_string() } else { role.clone() };
            let mut loc = if form.contains(&role_key.as_str()) && !name.is_empty() {
                serde_json::json!({ "label": name })
            } else if name.is_empty() {
                serde_json::json!({ "role": role_key })
            } else {
                serde_json::json!({ "role": role_key, "name": name })
            };
            if many || name.is_empty() {
                loc["nth"] = serde_json::json!(i);
            }
            SnapElement { kind: crate::config::ui::role_label(&role_key).to_string(), name, locator: loc, states }
        })
        .collect();
    let shot = page.screenshot().ok().and_then(|s| {
        let dir = snapshot_dir(root);
        let file = dir.join(format!("snapshot-{}.png", crate::history::now_ms()));
        crate::fsx::write(&file, &s.bytes).ok().map(|_| file)
    });
    Ok(UiSnapshot { location: info.location, title: info.title, elements, screenshot: shot })
}

/// adb 能看到的设备
pub struct AndroidDevices {
    /// (序列号, 状态：device / offline / unauthorized …)
    pub list: Vec<(String, String)>,
    /// 指定的网络设备没能连上的原因
    pub problem: Option<String>,
    /// 连接方式（远程 adb server / 经代理）
    pub via: Option<String>,
}

/// adb 能看到的设备与状态；指定了网络设备（ANDROID_SERIAL / android.serial 为 主机:端口）时先连接
pub fn android_devices(root: Option<&Path>) -> Result<AndroidDevices> {
    use crate::engine::android;
    let project = root.map(project).unwrap_or_default();
    let mut adb = android::Adb::from_config(&project.android, &default_proxy(root)?)?;
    let want = std::env::var("ANDROID_SERIAL").ok().filter(|s| !s.is_empty()).or(project.android.serial.clone());
    let problem = match want.filter(|w| android::is_network_serial(w)) {
        Some(w) => adb.actual_serial(&w).and_then(|actual| adb.ensure_connected(&actual)).err().map(|e| format!("{e:#}")),
        None => None,
    };
    Ok(AndroidDevices { list: adb.devices()?, problem, via: adb.via.clone() })
}

/// 读取设备当前界面，列出元素与推荐定位（id 唯一用 id，否则文字 / 描述）
pub fn android_snapshot(root: Option<&Path>) -> Result<UiSnapshot> {
    use crate::engine::android;
    use crate::engine::ui::UiDriver;
    let project = root.map(project).unwrap_or_default();
    let mut adb = android::Adb::from_config(&project.android, &default_proxy(root)?)?;
    let serial = adb.pick_serial(project.android.serial.as_deref())?;
    let mut dev = android::Device::connect(adb, serial, String::new(), None, std::time::Duration::from_secs(10))?;
    let info = dev.page()?;
    let nodes = dev.nodes()?;
    let count = |f: &dyn Fn(&android::Node) -> bool| nodes.iter().filter(|n| f(n)).count();
    let mut elements = vec![];
    for n in nodes.iter().filter(|n| n.visible && (n.clickable || !n.text.is_empty() || !n.desc.is_empty() || n.class.contains("EditText"))) {
        let short_id = n.id.rsplit('/').next().unwrap_or_default().to_string();
        let loc = if !short_id.is_empty() && count(&|x| x.id == n.id) == 1 {
            serde_json::json!({ "id": short_id })
        } else if !n.text.is_empty() && count(&|x| x.text == n.text) == 1 {
            serde_json::json!({ "text": n.text })
        } else if !n.desc.is_empty() && count(&|x| x.desc == n.desc) == 1 {
            serde_json::json!({ "desc": n.desc })
        } else if !short_id.is_empty() && !n.text.is_empty() {
            serde_json::json!({ "id": short_id, "text": n.text })
        } else {
            continue;
        };
        let class = n.class.rsplit('.').next().unwrap_or_default();
        let kind = if class.contains("EditText") {
            "输入框"
        } else if class.contains("CheckBox") {
            "复选框"
        } else if class.contains("Switch") {
            "开关"
        } else if class.contains("Button") {
            "按钮"
        } else if class.contains("Image") {
            "图片"
        } else if n.clickable {
            "可点击"
        } else {
            "文字"
        };
        let mut states = vec![];
        if !n.enabled {
            states.push("禁用".into());
        }
        if n.checkable && n.checked {
            states.push("已勾选".into());
        }
        let name = if !n.text.is_empty() { n.text.clone() } else if !n.desc.is_empty() { n.desc.clone() } else { n.hint.clone() };
        elements.push(SnapElement { kind: kind.into(), name, locator: loc, states });
    }
    let shot = dev.screenshot().ok().and_then(|s| {
        let file = snapshot_dir(root).join(format!("snapshot-{}.png", crate::history::now_ms()));
        crate::fsx::write(&file, &s.bytes).ok().map(|_| file)
    });
    Ok(UiSnapshot { location: info.location, title: info.title, elements, screenshot: shot })
}
