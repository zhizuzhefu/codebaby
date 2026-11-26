//! 环境自检：`codebaby doctor` 与 Desktop「环境自检」共用。
use crate::config::{load_strict, EnvFile, Project};
use crate::runner::discover;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckLevel {
    Ok,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub level: CheckLevel,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorReport {
    pub root: Option<PathBuf>,
    pub checks: Vec<Check>,
}

impl DoctorReport {
    pub fn ok(&self) -> bool {
        self.checks.iter().all(|c| c.level != CheckLevel::Error)
    }
}

/// `env`：检查 baseUrl 可达性所用的环境（缺省 defaultEnv）
pub fn run(start: &Path, env: Option<&str>) -> DoctorReport {
    let mut checks = vec![Check {
        level: CheckLevel::Ok,
        message: format!("HTTP 引擎 libcurl {}（静态链接，无需系统 curl）", crate::engine::libcurl_version()),
    }];
    let push = |checks: &mut Vec<Check>, level, message: String| checks.push(Check { level, message });

    let root = discover::find_project_root(start);
    match &root {
        None => push(&mut checks, CheckLevel::Warn, "当前不在项目内（无 codebaby.json）".into()),
        Some(root) => {
            push(&mut checks, CheckLevel::Ok, format!("项目根: {}", root.display()));
            match discover::project_file(root).map(|pf| load_strict::<Project>(&pf)) {
                Some(Ok(p)) => {
                    push(
                        &mut checks,
                        CheckLevel::Ok,
                        format!("项目配置 ok（defaultEnv={} workflows={}）", p.default_env, p.workflows.len()),
                    );
                    if !root.join("envs").join(format!("{}.json", p.default_env)).exists() {
                        push(&mut checks, CheckLevel::Warn, format!("默认环境 {} 没有对应的 envs/{}.json", p.default_env, p.default_env));
                    }
                }
                Some(Err(e)) => push(&mut checks, CheckLevel::Error, format!("项目配置解析失败: {e:#}")),
                None => {}
            }
            let envs_dir = root.join("envs");
            if envs_dir.is_dir() {
                let names = crate::config::list_envs(root);
                let mut locals = vec![];
                for env in &names {
                    for (p, local) in [(crate::config::env_path(root, env), false), (crate::config::local_env_path(root, env), true)] {
                        if !p.exists() {
                            continue;
                        }
                        match load_strict::<EnvFile>(&p) {
                            Ok(_) if local => locals.push(env.clone()),
                            Ok(_) => {}
                            Err(err) => push(&mut checks, CheckLevel::Error, format!("环境文件 {} 解析失败: {err:#}", p.display())),
                        }
                    }
                }
                push(&mut checks, CheckLevel::Ok, format!("环境文件 {} 个（{}）", names.len(), names.join(", ")));
                if !locals.is_empty() {
                    let ignored = crate::scaffold::local_env_ignored(root);
                    push(
                        &mut checks,
                        if ignored { CheckLevel::Ok } else { CheckLevel::Warn },
                        format!(
                            "本机覆盖 {}{}",
                            locals.iter().map(|e| format!("envs/{e}.local.json")).collect::<Vec<_>>().join(", "),
                            if ignored { "（已 gitignore）" } else { "——未被 .gitignore 忽略，密钥可能入库" }
                        ),
                    );
                }
            } else {
                push(&mut checks, CheckLevel::Warn, "无 envs/ 目录".into());
            }
            let proxy = check_proxy(root, env, &mut checks);
            match discover::discover(&[root.join("collections")]) {
                Ok(cases) => {
                    push(&mut checks, CheckLevel::Ok, format!("用例 {} 个", cases.len()));
                    check_ui(root, &cases, &proxy, &mut checks);
                }
                Err(e) => push(&mut checks, CheckLevel::Warn, format!("collections/ 不可读: {e:#}")),
            }
            check_base_url(root, env, &proxy, &mut checks);
            use crate::guide::AgentEntryStatus as A;
            match crate::guide::agent_entry_status(root) {
                A::Current => push(&mut checks, CheckLevel::Ok, "AGENTS.md 与已安装版本一致（AI 编码助手的入口）".into()),
                A::Outdated => push(
                    &mut checks,
                    CheckLevel::Warn,
                    "AGENTS.md 是旧版 codebaby 生成的入口，与已安装版本不一致：codebaby guide --init --update 更新".into(),
                ),
                A::Missing => push(&mut checks, CheckLevel::Warn, "没有 AGENTS.md：AI 编码助手不知道这里的约定（codebaby guide --init 生成）".into()),
                A::Custom => push(&mut checks, CheckLevel::Ok, "AGENTS.md 由你维护（不检查）".into()),
            }
            let runs = crate::history::list_paths(root).len();
            let dir = crate::history::runs_dir(root);
            let size = dir_size(&dir) + dir_size(&crate::history::scratch_dir(root));
            let dir = dir.strip_prefix(root).unwrap_or(&dir).display();
            let size = if size >= 1 << 20 { format!("，含截图证据共 {:.1} MB", size as f64 / (1 << 20) as f64) } else { String::new() };
            push(&mut checks, CheckLevel::Ok, format!("运行历史 {runs} 条（{dir}/{size}）"));
        }
    }
    DoctorReport { root, checks }
}

/// 目录总大小（字节）
fn dir_size(dir: &Path) -> u64 {
    walkdir::WalkDir::new(dir).into_iter().flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum()
}

/// 有界面用例时：Web 要有浏览器，Android 要有 adb 和一台可用设备，Mac 桌面要有 agent-desktop 和辅助功能权限
fn check_ui(root: &Path, cases: &[discover::DiscoveredCase], proxy: &crate::engine::proxy::ProxyPolicy, checks: &mut Vec<Check>) {
    use crate::config::{Protocol, TestCase};
    let protocols: Vec<Protocol> = cases.iter().filter_map(|c| load_strict::<TestCase>(&c.path).ok()).map(|c| c.protocol()).collect();
    let web = protocols.iter().filter(|p| **p == Protocol::Web).count();
    let android = protocols.iter().filter(|p| **p == Protocol::Android).count();
    let desktop = protocols.iter().filter(|p| **p == Protocol::Desktop).count();
    let project = discover::project_file(root).and_then(|p| load_strict::<Project>(&p).ok()).unwrap_or_default();
    if let (true, Some(ep)) = (web > 0, crate::engine::cdp::endpoint(project.web.endpoint.as_deref())) {
        let shown = crate::engine::cdp::redact_url(&ep);
        let c = match crate::engine::cdp::Browser::connect_remote(&ep, proxy, false, std::time::Duration::from_secs(10)) {
            Ok(b) => Check { level: CheckLevel::Ok, message: format!("远程浏览器 {shown}：{}（{web} 个 Web 用例）", b.product.replace('/', " ")) },
            Err(e) => Check { level: CheckLevel::Error, message: format!("{web} 个 Web 用例无法运行：{e:#}") },
        };
        checks.push(c);
    } else if web > 0 {
        let c = match crate::engine::cdp::find_browser(project.web.browser.as_deref()) {
            Ok(p) => {
                let version = std::process::Command::new(&p).arg("--version").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|v| !v.is_empty());
                Check { level: CheckLevel::Ok, message: format!("浏览器 {}（{web} 个 Web 用例）", version.unwrap_or_else(|| p.display().to_string())) }
            }
            Err(e) => Check { level: CheckLevel::Error, message: format!("{web} 个 Web 用例无法运行：{e:#}") },
        };
        checks.push(c);
    }
    if android > 0 {
        use crate::engine::android;
        let c = match android::Adb::from_config(&project.android, proxy).and_then(|mut adb| adb.pick_serial(project.android.serial.as_deref())) {
            Ok(serial) => Check { level: CheckLevel::Ok, message: format!("Android 设备 {serial}（{android} 个 Android 用例）") },
            Err(e) => Check { level: CheckLevel::Warn, message: format!("{android} 个 Android 用例暂时无法运行：{e:#}") },
        };
        checks.push(c);
    }
    if desktop > 0 {
        checks.extend(check_desktop(&project.desktop, desktop));
    }
    checks.extend(check_ocr(cases, &project));
}

/// 用到屏幕文字识别（`ocr` 定位）的用例：识别引擎要可用、要的语言要支持；Mac 桌面按文字点击还要允许真实鼠标
fn check_ocr(cases: &[discover::DiscoveredCase], project: &Project) -> Vec<Check> {
    use crate::config::{TestCase, UiAction, UiStep};
    let loaded: Vec<TestCase> = cases.iter().filter_map(|c| load_strict::<TestCase>(&c.path).ok()).collect();
    let steps_of = |c: &TestCase| -> Vec<UiStep> {
        c.web.as_ref().map(|s| s.steps.clone()).or_else(|| c.android.as_ref().map(|s| s.steps.clone())).or_else(|| c.desktop.as_ref().map(|s| s.steps.clone())).unwrap_or_default()
    };
    let step_ocr = |s: &UiStep| s.all_locators().iter().any(|l| l.is_ocr());
    let using: Vec<&TestCase> = loaded.iter().filter(|c| crate::config::flatten_steps(&steps_of(c)).into_iter().any(step_ocr)).collect();
    if using.is_empty() {
        return vec![];
    }
    let n = using.len();
    let mut out = vec![];
    match crate::engine::ocr::engine().and_then(|e| e.languages().map(|l| (e.name(), l))) {
        Ok((name, supported)) => {
            let want: Vec<String> = if project.ocr.languages.is_empty() { crate::engine::ocr::DEFAULT_LANGUAGES.iter().map(|s| s.to_string()).collect() } else { project.ocr.languages.clone() };
            let missing: Vec<&String> = want.iter().filter(|w| !supported.contains(w)).collect();
            if missing.is_empty() {
                out.push(Check { level: CheckLevel::Ok, message: format!("屏幕文字识别（OCR）：{name}，识别 {}（{n} 个用例使用）", want.join(" / ")) });
            } else {
                let miss = missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" / ");
                out.push(Check { level: CheckLevel::Error, message: format!("{n} 个用例用到屏幕文字识别，但 {name} 不支持识别语言 {miss}（支持：{}）：改 codebaby.json 的 ocr.languages；中文需要 macOS 13 及以上", supported.join(" / ")) });
            }
        }
        Err(e) => out.push(Check { level: CheckLevel::Error, message: format!("{n} 个用例用到屏幕文字识别（ocr），无法运行：{e:#}") }),
    }
    let acts = [UiAction::Click, UiAction::LongPress, UiAction::Hover, UiAction::Fill, UiAction::Select];
    let desktop_click = using.iter().filter(|c| c.desktop.is_some()).any(|c| c.desktop.as_ref().is_some_and(|d| crate::config::flatten_steps(&d.steps).into_iter().any(|s| s.locator().is_some_and(|l| l.is_ocr()) && s.action().is_some_and(|a| acts.contains(&a)))));
    if desktop_click && project.desktop.headed != Some(true) {
        out.push(Check { level: CheckLevel::Warn, message: "Mac 桌面用例按屏幕文字点击 / 填写，需要真实鼠标：codebaby.json 写 \"desktop\": { \"headed\": true }（运行时会移动鼠标、把窗口带到最前，期间不要动鼠标）".into() });
    }
    out
}

/// Mac 桌面：agent-desktop 要装好，辅助功能权限（截图还要屏幕录制权限）要给运行 codebaby 的程序
fn check_desktop(cfg: &crate::config::ui::DesktopConfig, n: usize) -> Vec<Check> {
    use crate::engine::desktop;
    if !cfg!(target_os = "macos") {
        return vec![Check { level: CheckLevel::Error, message: format!("{n} 个 Mac 桌面用例只能在 macOS 上运行") }];
    }
    let ad = match desktop::AgentDesktop::from_config(cfg) {
        Ok(a) => a,
        Err(e) => return vec![Check { level: CheckLevel::Error, message: format!("{n} 个 Mac 桌面用例无法运行：{e:#}") }],
    };
    let version = std::process::Command::new(&ad.path).arg("--version").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|v| !v.is_empty());
    let mut out = vec![Check { level: CheckLevel::Ok, message: format!("{}（{n} 个 Mac 桌面用例）", version.unwrap_or_else(|| ad.path.display().to_string())) }];
    match ad.call(&["permissions"], std::time::Duration::from_secs(10)) {
        Ok(p) => {
            let state = |k: &str| p.pointer(&format!("/{k}/state")).and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
            let (ax, rec) = (state("accessibility"), state("screen_recording"));
            if ax != "granted" {
                out.push(Check { level: CheckLevel::Error, message: "没有辅助功能权限：系统设置 → 隐私与安全性 → 辅助功能，给运行 codebaby 的程序（终端 / codebaby.app）打开；agent-desktop permissions --guide 会带你操作".into() });
            }
            if rec != "granted" {
                out.push(Check { level: CheckLevel::Warn, message: "没有屏幕录制权限：用例能跑，但截图会失败（系统设置 → 隐私与安全性 → 屏幕录制）".into() });
            }
            if ax == "granted" && rec == "granted" {
                out.push(Check { level: CheckLevel::Ok, message: "辅助功能与屏幕录制权限已授权".into() });
            }
        }
        Err(e) => out.push(Check { level: CheckLevel::Warn, message: format!("读取 agent-desktop 权限失败：{e:#}") }),
    }
    out
}

/// 代理：说明当前规则；启用时确认代理本身连得上、账号密码对
fn check_proxy(root: &Path, env: Option<&str>, checks: &mut Vec<Check>) -> crate::engine::proxy::ProxyPolicy {
    use crate::engine::proxy::ProxyPolicy;
    let env = env.map(String::from).unwrap_or_else(|| crate::session::resolve_env(&[root.to_path_buf()], None));
    let policy = match crate::runner::load_project_ctx(root, &env) {
        Ok(ctx) => ctx.proxy,
        Err(e) => {
            checks.push(Check { level: CheckLevel::Error, message: format!("代理配置有误：{e:#}") });
            return ProxyPolicy::Off;
        }
    };
    let from = if std::env::var(crate::engine::proxy::ENV_OVERRIDE).is_ok_and(|v| !v.trim().is_empty()) { "（CODEBABY_PROXY）" } else { "" };
    let c = match &policy {
        ProxyPolicy::On(p) => match crate::engine::proxy::probe(p, std::time::Duration::from_secs(5)) {
            Ok(()) => Check { level: CheckLevel::Ok, message: format!("代理 {}{from}：可连接，所有对外连接经它", policy.describe()) },
            Err(e) => Check { level: CheckLevel::Error, message: format!("代理 {}{from} 不可用：{e:#}", p.display()) },
        },
        other => Check { level: CheckLevel::Ok, message: format!("代理：{}{from}", other.describe()) },
    };
    checks.push(c);
    policy
}

/// 当前环境的 baseUrl 是否可达（被测服务由使用者自行启动）
fn check_base_url(root: &Path, env: Option<&str>, proxy: &crate::engine::proxy::ProxyPolicy, checks: &mut Vec<Check>) {
    let env = env.map(String::from).unwrap_or_else(|| crate::session::resolve_env(&[root.to_path_buf()], None));
    let base = crate::config::load_env(root, &env).ok().flatten().and_then(|e| e.vars.get("baseUrl").map(|v| v.value.clone()));
    let Some(base) = base.filter(|b| b.starts_with("http") && !b.contains("{{")) else { return };
    let req = crate::engine::ResolvedRequest { method: "GET".into(), url: base.clone(), timeout_ms: 2_000, proxy: proxy.clone(), ..Default::default() };
    use crate::engine::Transport;
    let (level, message) = match crate::engine::CurlTransport::new().execute(&req) {
        Ok(r) if r.status > 0 => (CheckLevel::Ok, format!("baseUrl {base} 可达（环境 {env}，HTTP {}）", r.status)),
        _ => (CheckLevel::Warn, format!("baseUrl {base} 不可达（环境 {env}）：被测服务没启动，或地址不对（改地址：codebaby vars set baseUrl http://主机:端口 -e {env}）")),
    };
    checks.push(Check { level, message });
}
