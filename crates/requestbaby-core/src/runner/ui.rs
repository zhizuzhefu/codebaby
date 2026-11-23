//! 界面用例（Web / Android / Mac 桌面）的步骤循环：各驱动共用。
//!
//! 每一步：替换变量 → 自动等待目标（出现、可见、可用、位置稳定、没被遮挡）→ 操作 → 等界面稳定 → 截图留证。
//! 点击类截图在操作前拍并标出点击位置（红框 + 圆点），输入类在操作后拍并框出输入框，
//! 断言 / 等待通过时框出验证到的元素（绿框）；失败时另存现场（页面结构、控制台、网络 / logcat）。
use super::{discover, leftover_error, CaseResult, Exchanged, RunEvent, RunOptions, Runner};
use crate::config::ui::ScreenshotPolicy;
use crate::config::{CaptureSource, Locator, Protocol, TestCase, UiAction, UiKind, UiState, UiStep};
use crate::engine::cdp::{self, Browser};
use crate::engine::ui::{self as eui, Actionable, Element, Mark, MarkKind, StepRecord, UiDriver, UiEvidence, UiStepStatus};
use crate::script::{ReqData, RespData, TestOutcome};
use crate::vars::{VarVal, Vars};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// 每一步等待的缺省超时
pub const DEFAULT_STEP_TIMEOUT_MS: u64 = 10_000;

/// 一次运行里共享的界面资源：浏览器只启动一次（每个用例一个独立上下文），APK 只装一次
#[derive(Default)]
pub struct UiResources {
    browser: Option<Browser>,
    installed: Vec<PathBuf>,
    /// 不在项目里运行时的证据目录（临时目录）
    fallback_dir: Option<PathBuf>,
}

/// 证据目录（`.requestbaby/runs/<运行 ID>/`），由 session / workflow / verify 设置
#[derive(Debug, Clone)]
pub struct ArtifactsDir {
    /// 项目根：证据路径相对它记录
    pub root: PathBuf,
    /// 本次运行的证据目录（绝对路径）
    pub dir: PathBuf,
}

/// 打开页面 / 启动应用（步骤 0）
enum Open {
    Goto(String),
    Launch(bool),
    /// 接管设备当前的界面（Android attach）：不启动、不关闭
    Attach,
}

/// 一步的执行结果
struct Done {
    /// 截图上的标记；None = 不截图（静候）
    marks: Option<Vec<Mark>>,
    /// 操作前已拍好的截图（点击类）
    shot: Option<String>,
    detail: Option<String>,
    skipped: bool,
    /// 断言通过（计入断言结果）
    asserted: bool,
    /// 替换步骤描述（填写密码框：不写出值）
    title: Option<String>,
}

/// 一步的失败：原因 + 失败截图上要标出的元素
struct Failed {
    why: String,
    marks: Vec<Mark>,
}

impl Failed {
    fn new(why: impl Into<String>) -> Self {
        Failed { why: why.into(), marks: vec![] }
    }
}

/// 等元素失败的原因
enum WaitErr {
    /// 超时没等到（optional 步骤据此跳过）
    Timeout(String, Option<Box<Element>>),
    /// 写法有误 / 匹配到多个等不会因为等待而改变的（附带要在截图上标出的元素）
    Fatal(String, Vec<eui::Rect>),
}

fn render_locator(l: &Locator, v: &Vars) -> Locator {
    let r = |o: &Option<String>| o.as_ref().map(|s| v.render(s));
    Locator {
        role: r(&l.role),
        name: r(&l.name),
        label: r(&l.label),
        text: r(&l.text),
        placeholder: r(&l.placeholder),
        test_id: r(&l.test_id),
        css: r(&l.css),
        id: r(&l.id),
        desc: r(&l.desc),
        url: r(&l.url),
        title: r(&l.title),
        activity: r(&l.activity),
        value: r(&l.value),
        has_text: r(&l.has_text),
        ocr: r(&l.ocr),
        row: r(&l.row),
        attr: r(&l.attr),
        regex: l.regex.clone(),
        ..l.clone()
    }
}

/// 步骤的人话描述（变量已替换）：点击 按钮「登录」/ 填写 输入框「手机号」= 138…
pub fn describe_ui_step(s: &UiStep, kind: UiKind) -> String {
    let Some(a) = s.action() else { return "（每一步只能写一个动作）".into() };
    let target = s.locator().map(|l| l.describe()).unwrap_or_default();
    let clip = |t: &str| {
        let one = t.replace('\n', " ");
        if one.chars().count() > 40 {
            format!("{}…", one.chars().take(40).collect::<String>())
        } else {
            one
        }
    };
    let mut d = match a {
        UiAction::Goto => format!("打开 {}", s.goto.clone().unwrap_or_default()),
        UiAction::Click | UiAction::LongPress | UiAction::Hover => format!("{} {target}", if kind == UiKind::Android && a == UiAction::Click { "点按" } else { a.label() }),
        UiAction::Fill => format!("填写 {target} = 「{}」", clip(s.value.as_deref().unwrap_or_default())),
        UiAction::Select => format!("在{} 选择「{}」", target.replacen("输入框", "下拉框", 1), s.value.clone().unwrap_or_default()),
        UiAction::Type => format!("输入「{}」", clip(s.type_text.as_deref().unwrap_or_default())),
        UiAction::Press => format!("按键 {}", s.press.clone().unwrap_or_default()),
        UiAction::Swipe => format!("向{}滑动", match s.swipe.as_deref().unwrap_or_default() { "up" => "上", "down" => "下", "left" => "左", "right" => "右", other => other }),
        UiAction::Back => "返回".into(),
        UiAction::Launch => match (kind, s.clear_data == Some(true)) {
            (UiKind::Desktop, _) => "重新启动应用".into(),
            (_, true) => "清除数据后启动应用".into(),
            _ => "启动应用".into(),
        },
        UiAction::Stop => "关闭应用".into(),
        UiAction::WaitFor | UiAction::Expect => format!("{} {}", a.label(), s.locator().map(|l| l.describe_condition()).unwrap_or_default()),
        UiAction::Screenshot => format!("截图「{}」", s.screenshot.clone().unwrap_or_default()),
        UiAction::Wait => format!("静候 {} ms", s.wait.unwrap_or(0)),
    };
    if s.optional == Some(true) {
        d.push_str("（可选）");
    }
    if !s.capture.is_empty() {
        d.push_str(&format!("，捕获 {}", s.capture.keys().cloned().collect::<Vec<_>>().join("、")));
    }
    d
}

fn file_slug(s: &str) -> String {
    let out: String = s.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(40).collect();
    out.trim_matches('_').to_string()
}

/// 按定位找元素：`ocr` 对截图做文字识别（三端通用），其余读驱动自己的界面结构
fn find_all(d: &mut dyn UiDriver, loc: &Locator) -> anyhow::Result<Vec<Element>> {
    if loc.is_ocr() {
        crate::engine::ocr_match::find(d, loc)
    } else {
        d.find(loc)
    }
}

/// 这一步有没有用到屏幕文字识别（定位或捕获里写了 `ocr`）
fn uses_ocr(step: &UiStep) -> bool {
    step.locator().is_some_and(|l| l.is_ocr()) || step.capture.values().any(|c| matches!(c, CaptureSource::Element(l) if l.is_ocr()))
}

/// 等到唯一的目标元素（need_action：还要可用、位置稳定、没被遮挡）
fn wait_element(d: &mut dyn UiDriver, loc: &Locator, deadline: Instant, need_action: bool, cancelled: &dyn Fn() -> bool) -> Result<Element, WaitErr> {
    let desc = loc.describe();
    let poll = d.kind().poll();
    let mut why = String::new();
    let mut last: Option<Element>;
    // 屏幕文字没有「位置是否稳定」的信息：要连续两次识别在同一位置才操作
    let mut prev_visual: Option<eui::Rect> = None;
    loop {
        let els = find_all(d, loc).map_err(|e| WaitErr::Fatal(format!("{e:#}"), vec![]))?;
        let total = els.len();
        let mut vis: Vec<Element> = els.into_iter().filter(|e| e.visible).collect();
        if let Some(t) = &loc.has_text {
            vis.retain(|e| eui::text_matches(&e.text, t, false));
        }
        last = None;
        let target = match loc.nth {
            Some(n) => match vis.get(n) {
                Some(e) => Some(e.clone()),
                None => {
                    why = format!("只找到 {} 个可见的{desc}，没有第 {} 个", vis.len(), n + 1);
                    None
                }
            },
            None => match vis.len() {
                0 => {
                    why = if total > 0 { format!("找到了{desc}，但不可见") } else { format!("没有找到{desc}") };
                    None
                }
                1 => Some(vis.remove(0)),
                k => {
                    let texts: Vec<String> = vis.iter().take(5).map(|e| if e.text.is_empty() { e.tag.clone() } else { format!("「{}」", e.text.chars().take(30).collect::<String>().replace('\n', " ")) }).collect();
                    let rects = vis.iter().take(20).map(|e| e.rect).collect();
                    return Err(WaitErr::Fatal(format!("找到 {k} 个{desc}（{}{}）：加 \"nth\" 指定第几个（从 0 开始），或把名称写完整并加 \"exact\": true", texts.join("、"), if k > 5 { "…" } else { "" }), rects));
                }
            },
        };
        if target.is_none() {
            prev_visual = None;
        }
        if let Some(el) = target {
            if !need_action {
                return Ok(el);
            }
            if el.visual {
                let steady = prev_visual.is_some_and(|p| (p.x - el.rect.x).abs() <= 3.0 && (p.y - el.rect.y).abs() <= 3.0);
                if steady {
                    return Ok(el);
                }
                prev_visual = Some(el.rect);
                why = format!("{desc}的位置还在变化（动画中）");
                last = Some(el);
            } else if !el.enabled {
                why = format!("{desc}是禁用状态");
                last = Some(el);
            } else {
                match d.actionable(&el).map_err(|e| WaitErr::Fatal(format!("{e:#}"), vec![]))? {
                    Actionable::Ok(el) => return Ok(el),
                    Actionable::NotYet(w) => {
                        why = format!("{desc}{w}");
                        last = Some(el);
                    }
                }
            }
        }
        if Instant::now() >= deadline || cancelled() {
            return Err(WaitErr::Timeout(why, last.map(Box::new)));
        }
        std::thread::sleep(poll);
    }
}

/// 条件检查一次：(是否满足, 不满足的原因, 验证到的元素位置)
fn check_once(d: &mut dyn UiDriver, loc: &Locator) -> anyhow::Result<(bool, String, Vec<eui::Rect>)> {
    let exact = loc.exact.unwrap_or(false);
    let mut problems: Vec<String> = vec![];
    let mut rects = vec![];
    if loc.url.is_some() || loc.title.is_some() || loc.activity.is_some() {
        let p = d.page()?;
        if let Some(u) = &loc.url {
            if !(if exact { p.location == *u } else { p.location.contains(u.as_str()) }) {
                problems.push(format!("当前地址是 {}", p.location));
            }
        }
        if let Some(t) = &loc.title {
            if !eui::text_matches(&p.title, t, exact) {
                problems.push(format!("标题是「{}」", p.title));
            }
        }
        if let Some(a) = &loc.activity {
            if !p.title.contains(a.as_str()) {
                problems.push(format!("当前界面是 {}（{}）", if p.title.is_empty() { "未知" } else { &p.title }, p.location));
            }
        }
    }
    if loc.has_target() {
        let desc = loc.describe();
        let mut find = loc.clone();
        find.has_text = None;
        let els = find_all(d, &find)?;
        let total = els.len();
        let vis: Vec<Element> = els.into_iter().filter(|e| e.visible).collect();
        let matching: Vec<Element> = match &loc.has_text {
            Some(t) => vis.iter().filter(|e| eui::text_matches(&e.text, t, false)).cloned().collect(),
            None => vis.clone(),
        };
        if let Some(n) = loc.count {
            if !n.matches(matching.len()) {
                problems.push(format!("实际有 {} 个", matching.len()));
            }
            rects.extend(matching.iter().take(20).map(|e| e.rect));
        } else if loc.state == Some(UiState::Hidden) {
            if let Some(e) = matching.first() {
                problems.push(format!("{desc}仍然可见"));
                rects.push(e.rect);
            }
        } else {
            let target = match loc.nth {
                Some(n) => matching.get(n).cloned(),
                None => matching.first().cloned(),
            };
            match target {
                None => problems.push(match (&loc.has_text, vis.first()) {
                    (Some(_), Some(e)) => format!("{desc}的文字是「{}」", e.text.chars().take(80).collect::<String>()),
                    _ if total > 0 => format!("找到了{desc}，但不可见"),
                    _ if loc.nth.is_some() && !matching.is_empty() => format!("只找到 {} 个{desc}", matching.len()),
                    _ => format!("没有找到{desc}"),
                }),
                Some(el) => {
                    let state_ok = match loc.state {
                        Some(UiState::Enabled) => el.enabled,
                        Some(UiState::Disabled) => !el.enabled,
                        Some(UiState::Checked) => el.checked == Some(true),
                        Some(UiState::Unchecked) => el.checked == Some(false),
                        _ => true,
                    };
                    if !state_ok {
                        let actual = match loc.state {
                            Some(UiState::Enabled | UiState::Disabled) => if el.enabled { "可用" } else { "禁用" }.to_string(),
                            _ => match el.checked {
                                Some(true) => "已勾选".into(),
                                Some(false) => "未勾选".into(),
                                None => "不是可勾选的元素".into(),
                            },
                        };
                        problems.push(format!("{desc}实际{actual}"));
                    }
                    if let Some(want) = &loc.value {
                        let actual = el.value.clone().unwrap_or_else(|| el.text.clone());
                        if !(if exact { actual == *want } else { actual.contains(want.as_str()) }) {
                            problems.push(format!("{desc}的值是「{actual}」"));
                        }
                    }
                    if loc.nth.is_none() && loc.state.is_none() && loc.value.is_none() {
                        rects.extend(matching.iter().take(20).map(|e| e.rect));
                    } else {
                        rects.push(el.rect);
                    }
                }
            }
        }
    }
    Ok((problems.is_empty(), problems.join("；"), rects))
}

impl Runner {
    /// 本用例的证据目录：(绝对路径, 记录用的路径)
    fn evidence_dir(&self, dc: &discover::DiscoveredCase, name: &str, opts: &RunOptions) -> (PathBuf, String) {
        let (base, root) = match &opts.artifacts {
            Some(a) => (a.dir.clone(), Some(a.root.clone())),
            None => {
                let mut ui = self.ui.lock().unwrap_or_else(|e| e.into_inner());
                let d = ui
                    .fallback_dir
                    .get_or_insert_with(|| std::env::temp_dir().join("requestbaby-evidence").join(format!("{}-{}", crate::history::now_ms(), std::process::id())))
                    .clone();
                (d, None)
            }
        };
        let stem = eui::case_dir_name(&dc.rel, name);
        let mut dir = base.join(&stem);
        let mut k = 2;
        while dir.exists() {
            dir = base.join(format!("{stem}-{k}"));
            k += 1;
        }
        let display = root
            .as_ref()
            .and_then(|r| dir.strip_prefix(r).ok())
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|| dir.display().to_string());
        (dir, display)
    }

    /// Web：启动（或复用）浏览器 → 新建独立上下文 → 打开 url → 逐步执行
    #[allow(clippy::too_many_arguments)]
    pub(super) fn exchange_web(
        &mut self,
        dc: &discover::DiscoveredCase,
        case: &TestCase,
        row: Option<&crate::config::DataRow>,
        vars: &Vars,
        env_view: &BTreeMap<String, VarVal>,
        result: &mut CaseResult,
        opts: &RunOptions,
    ) -> Option<Exchanged> {
        let spec = case.web.as_ref().expect("Web 用例");
        result.method = "WEB".into();
        let mut url = vars.render(&spec.url);
        if !case.pre.is_empty() {
            let req = ReqData { method: "GET".into(), url: url.clone(), headers: Default::default(), body: None };
            if let Some(rd) = self.run_pre_script(dc, case, env_view, req, result)? {
                url = rd.url;
            }
            url = self.vars_for(&dc.chain).render(&url);
        }
        result.url = url.clone();
        if opts.dry_run {
            result.body_preview = Some(super::clip_preview(&describe_steps(&spec.steps, vars, UiKind::Web)));
            self.redact_result(result);
            return None;
        }
        if let Some(e) = leftover_error(std::slice::from_ref(&url)) {
            result.error = Some(e);
            return None;
        }
        let cfg = self.ctx.project.web.clone();
        let headed = opts.headed || cfg.headed.unwrap_or(false) || std::env::var("REQUESTBABY_HEADED").is_ok_and(|v| v == "1" || v == "true");
        let viewport = spec.viewport.or(cfg.viewport).unwrap_or_default();
        let nav_timeout = Duration::from_millis(self.ctx.project.timeout_ms.max(1000));
        let res = self.ui.clone();
        let mut guard = res.lock().unwrap_or_else(|e| e.into_inner());
        // 复用上一个用例的浏览器：进程还在、调试连接还通、界面模式 / 远程地址 / 代理相同
        let remote = cdp::endpoint(cfg.endpoint.as_deref()).map(|e| cdp::redact_url(&e));
        let reuse = guard.browser.as_mut().is_some_and(|b| {
            b.alive() && b.headed == headed && b.remote == remote && b.proxy == self.ctx.proxy && b.cdp.call(None, "Browser.getVersion", serde_json::json!({}), Duration::from_secs(5)).is_ok()
        });
        if !reuse {
            guard.browser = None;
            let launched = cdp::open_browser(&cfg, &self.ctx.proxy, headed, Duration::from_secs(30));
            match launched {
                Ok(b) => guard.browser = Some(b),
                Err(e) => {
                    result.error = Some(format!("{e:#}"));
                    return None;
                }
            }
        }
        let browser = guard.browser.as_mut().unwrap();
        let mut page = match crate::engine::web::WebPage::open(browser, (viewport.width, viewport.height), nav_timeout) {
            Ok(p) => p,
            Err(e) => {
                result.error = Some(format!("打开浏览器标签页失败：{e:#}"));
                return None;
            }
        };
        if self.ctx.project.cookies {
            let jar: Vec<crate::cookies::Cookie> = self.cookies.iter().cloned().collect();
            let _ = page.set_cookies(&jar);
            // 登录令牌在 localStorage / sessionStorage 里的应用：一并沿用
            let _ = page.inject_storage(&self.storage);
        }
        let (dir, display) = self.evidence_dir(dc, &result.name, opts);
        let policy = cfg.screenshots.unwrap_or_default();
        let step_timeout = spec.timeout_ms.unwrap_or(DEFAULT_STEP_TIMEOUT_MS);
        let (outcomes, evidence) = self.ui_steps(&mut page, dc, row, &spec.steps, Open::Goto(url), step_timeout, &dir, display, policy, opts);
        if self.ctx.project.cookies {
            let now = crate::history::now_ms();
            for c in page.cookies() {
                if c.expires_at_ms.is_none_or(|t| t > now) {
                    self.cookies.insert(c);
                }
            }
            if let Some((origin, st)) = page.read_storage() {
                self.storage.insert(origin, st);
            }
        }
        let info = page.page().unwrap_or_default();
        let resp = RespData {
            status: page.doc_status,
            time_ms: page.elapsed_ms(),
            size: 0,
            headers: page.doc_headers.clone(),
            body: serde_json::json!({ "url": info.location, "title": info.title }).to_string(),
            protocol: Protocol::Web,
            frames: vec![],
            evidence: Some(evidence),
        };
        drop(page);
        Some(Exchanged { resp, outcomes })
    }

    /// Android：选定设备 →（安装 APK）→ 启动应用 → 逐步执行
    #[allow(clippy::too_many_arguments)]
    pub(super) fn exchange_android(
        &mut self,
        dc: &discover::DiscoveredCase,
        case: &TestCase,
        row: Option<&crate::config::DataRow>,
        vars: &Vars,
        env_view: &BTreeMap<String, VarVal>,
        result: &mut CaseResult,
        opts: &RunOptions,
    ) -> Option<Exchanged> {
        use crate::engine::android;
        let spec = case.android.as_ref().expect("Android 用例");
        result.method = "APP".into();
        let _ = env_view;
        let app = vars.render(&spec.app);
        let activity = spec.activity.as_ref().map(|a| vars.render(a));
        result.url = match &activity {
            Some(a) => format!("{app}/{a}"),
            None => app.clone(),
        };
        if opts.dry_run {
            result.body_preview = Some(super::clip_preview(&describe_steps(&spec.steps, vars, UiKind::Android)));
            self.redact_result(result);
            return None;
        }
        if let Some(e) = leftover_error(&[app.clone(), activity.clone().unwrap_or_default()]) {
            result.error = Some(e);
            return None;
        }
        let cfg = self.ctx.project.android.clone();
        let started = Instant::now();
        let prepared = (|| -> anyhow::Result<android::Device> {
            let mut adb = android::Adb::from_config(&cfg, &self.ctx.proxy)?;
            let serial = adb.pick_serial(cfg.serial.as_deref())?;
            let launch_timeout = Duration::from_millis(self.ctx.project.timeout_ms.max(5000));
            let dev = android::Device::connect(adb, serial, app.clone(), activity.clone(), launch_timeout)?;
            if let Some(apk) = &spec.apk {
                let p = vars.render(apk);
                let root = discover::find_project_root(&dc.path).unwrap_or_else(|| PathBuf::from("."));
                let path = if Path::new(&p).is_absolute() { PathBuf::from(&p) } else { root.join(&p) };
                if !path.is_file() {
                    anyhow::bail!("APK 不存在：{}", path.display());
                }
                let mut ui = self.ui.lock().unwrap_or_else(|e| e.into_inner());
                if !ui.installed.contains(&path) {
                    self.emit(RunEvent::Log { file: dc.rel.clone(), line: format!("安装 {}", path.display()) });
                    dev.install(&path).map_err(|e| anyhow::anyhow!("安装 APK 失败：{e:#}"))?;
                    ui.installed.push(path);
                }
            }
            Ok(dev)
        })();
        let mut dev = match prepared {
            Ok(d) => d,
            Err(e) => {
                result.error = Some(format!("{e:#}"));
                result.duration_ms = started.elapsed().as_millis() as u64;
                return None;
            }
        };
        let (dir, display) = self.evidence_dir(dc, &result.name, opts);
        let policy = cfg.screenshots.unwrap_or_default();
        let step_timeout = spec.timeout_ms.unwrap_or(DEFAULT_STEP_TIMEOUT_MS);
        let (outcomes, evidence) = self.ui_steps(&mut dev, dc, row, &spec.steps, if spec.attach == Some(true) { Open::Attach } else { Open::Launch(spec.clear_data.unwrap_or(false)) }, step_timeout, &dir, display, policy, opts);
        let info = dev.page().unwrap_or_default();
        let resp = RespData {
            status: 0,
            time_ms: started.elapsed().as_millis() as u64,
            size: 0,
            headers: vec![],
            body: serde_json::json!({ "package": info.location, "activity": info.title }).to_string(),
            protocol: Protocol::Android,
            frames: vec![],
            evidence: Some(evidence),
        };
        Some(Exchanged { resp, outcomes })
    }

    /// Mac 桌面：找到 agent-desktop → 启动（或接管）应用 → 逐步执行
    pub(super) fn exchange_desktop(
        &mut self,
        dc: &discover::DiscoveredCase,
        case: &TestCase,
        row: Option<&crate::config::DataRow>,
        vars: &Vars,
        result: &mut CaseResult,
        opts: &RunOptions,
    ) -> Option<Exchanged> {
        use crate::engine::desktop;
        let spec = case.desktop.as_ref().expect("Mac 桌面用例");
        result.method = "MAC".into();
        let app = vars.render(&spec.app);
        let window = spec.window.as_ref().map(|w| vars.render(w));
        result.url = match &window {
            Some(w) => format!("{app} · {w}"),
            None => app.clone(),
        };
        if opts.dry_run {
            result.body_preview = Some(super::clip_preview(&describe_steps(&spec.steps, vars, UiKind::Desktop)));
            self.redact_result(result);
            return None;
        }
        if let Some(e) = leftover_error(&[app.clone(), window.clone().unwrap_or_default()]) {
            result.error = Some(e);
            return None;
        }
        let cfg = self.ctx.project.desktop.clone();
        let started = Instant::now();
        let ad = match desktop::AgentDesktop::from_config(&cfg) {
            Ok(a) => a,
            Err(e) => {
                result.error = Some(format!("{e:#}"));
                return None;
            }
        };
        let launch_timeout = Duration::from_millis(self.ctx.project.timeout_ms.max(5000));
        let mut app_driver = desktop::App::new(ad, app, window, launch_timeout);
        let (dir, display) = self.evidence_dir(dc, &result.name, opts);
        let policy = cfg.screenshots.unwrap_or_default();
        let step_timeout = spec.timeout_ms.unwrap_or(DEFAULT_STEP_TIMEOUT_MS);
        let (outcomes, evidence) = self.ui_steps(&mut app_driver, dc, row, &spec.steps, Open::Launch(spec.restart.unwrap_or(false)), step_timeout, &dir, display, policy, opts);
        let info = app_driver.page().unwrap_or_default();
        let resp = RespData {
            status: 0,
            time_ms: started.elapsed().as_millis() as u64,
            size: 0,
            headers: vec![],
            body: serde_json::json!({ "app": info.location, "window": info.title }).to_string(),
            protocol: Protocol::Desktop,
            frames: vec![],
            evidence: Some(evidence),
        };
        Some(Exchanged { resp, outcomes })
    }

    /// 步骤循环（各驱动共用）
    #[allow(clippy::too_many_arguments)]
    fn ui_steps(
        &mut self,
        d: &mut dyn UiDriver,
        dc: &discover::DiscoveredCase,
        row: Option<&crate::config::DataRow>,
        steps: &[UiStep],
        open: Open,
        default_timeout: u64,
        dir: &Path,
        display: String,
        policy: ScreenshotPolicy,
        opts: &RunOptions,
    ) -> (Vec<TestOutcome>, UiEvidence) {
        let kind = d.kind();
        let started = Instant::now();
        crate::engine::ocr::set_languages(self.ctx.project.ocr.languages.clone());
        let mut ocr_failed = false;
        let mut ev = UiEvidence { dir: display, ..Default::default() };
        let mut outcomes: Vec<TestOutcome> = vec![];
        let _ = std::fs::create_dir_all(dir);
        let all_shots = policy == ScreenshotPolicy::Steps;
        let rel = dc.rel.clone();
        let cancel = opts.cancel.clone();
        let cancelled = move || cancel.as_ref().is_some_and(|c| c.is_cancelled());

        // 截图并标注，保存为 dir/name；失败不影响用例结论（记一条说明）。
        // reuse：本步只是看（断言 / 等待）、刚用屏幕文字识别截过图，就直接用那一张（不改变画面，省一次截图）
        let snap = |d: &mut dyn UiDriver, name: &str, marks: &[Mark], reuse: bool| -> Result<String, String> {
            let reused = if reuse { crate::engine::ocr_match::take_fresh_shot(Duration::from_millis(1500)) } else { None };
            let shot = match reused {
                Some(s) => s,
                None => d.screenshot().map_err(|e| format!("截图失败：{e:#}"))?,
            };
            let jpg = eui::annotate(&shot, marks).map_err(|e| format!("{e:#}"))?;
            crate::fsx::write(dir.join(name), &jpg).map_err(|e| format!("保存截图失败：{e:#}"))?;
            Ok(name.to_string())
        };

        // ---- 步骤 0：打开页面 / 启动应用 ----
        let (open_title, open_res) = match &open {
            Open::Goto(u) => (format!("打开 {u}"), d.goto(u)),
            Open::Attach => ("接管当前界面（不启动应用）".to_string(), d.attach()),
            Open::Launch(clear) => {
                let title = match (kind, *clear) {
                    (UiKind::Desktop, true) => "重新启动应用",
                    (UiKind::Desktop, false) => "启动应用（已在运行则接管）",
                    (_, true) => "清除数据后启动应用",
                    _ => "启动应用",
                };
                (title.to_string(), d.launch(*clear))
            }
        };
        if open_res.is_ok() {
            d.settle(Duration::from_millis(default_timeout));
        }
        // 运行环境在打开之后读：桌面应用的版本、窗口要启动后才知道
        ev.environment = d.environment();
        let t_open = started.elapsed().as_millis() as u64;
        let open_failed = open_res.is_err();
        let shot = if all_shots || open_failed { snap(d, "00-open.jpg", &[], false).ok() } else { None };
        let mut rec = StepRecord {
            index: 0,
            action: "open".into(),
            title: open_title.clone(),
            status: if open_failed { UiStepStatus::Failed } else { UiStepStatus::Passed },
            at_ms: 0,
            duration_ms: t_open,
            screenshot: shot,
            detail: None,
        };
        self.emit(RunEvent::Log { file: rel.clone(), line: self.redact_str(&format!("{} #0 {open_title}  {t_open}ms", if open_failed { "✘" } else { "✔" })) });
        if let Err(e) = open_res {
            let why = format!("{e:#}");
            rec.detail = Some(why.clone());
            self.emit_ui_step(&rel, &ev.dir, &rec);
            ev.steps.push(rec);
            outcomes.push(TestOutcome { expr: format!("#0 {open_title}"), pass: false, detail: Some(why) });
            ev.artifacts = d.failure_scene(dir);
            return (outcomes, ev);
        }
        self.emit_ui_step(&rel, &ev.dir, &rec);
        ev.steps.push(rec);

        // ---- 逐步执行 ----
        let mut failed = false;
        for (i, raw) in steps.iter().enumerate() {
            if cancelled() {
                break;
            }
            let n = i + 1;
            let v = {
                let mut v = self.vars_for(&dc.chain);
                if let Some(row) = row {
                    v.push_layer(row.clone());
                }
                v
            };
            let step = render_step(raw, &v);
            let step_ocr = uses_ocr(&step);
            // 只看不动（断言 / 等待）：证据截图可以直接用屏幕文字识别刚用的那一张
            let observe_only = step_ocr && matches!(step.action(), Some(UiAction::WaitFor | UiAction::Expect));
            if step_ocr {
                let _ = crate::engine::ocr_match::take_note();
            }
            let title = describe_ui_step(&step, kind);
            let at = started.elapsed().as_millis() as u64;
            let timeout = Duration::from_millis(step.timeout_ms.unwrap_or(default_timeout).max(1));
            let deadline = Instant::now() + timeout;
            let key = step.action().map(|a| a.key(kind)).unwrap_or("step");
            let shot_name = |suffix: &str| format!("{n:02}-{suffix}.jpg");

            let res: Result<Done, Failed> = (|| {
                let Some(action) = step.action() else {
                    return Err(Failed::new("每一步只能写一个动作"));
                };
                if !action.supported(kind) {
                    return Err(Failed::new(format!("{} 用例不支持 {}", kind.label(), action.key(kind))));
                }
                if let Some(e) = leftover_error(&step.texts()) {
                    return Err(Failed::new(e));
                }
                let optional = step.optional == Some(true);
                let wait_err = |e: WaitErr| -> Result<Done, Failed> {
                    match e {
                        WaitErr::Timeout(why, _) if optional => Ok(Done { marks: Some(vec![]), shot: None, detail: Some(format!("跳过：{why}")), skipped: true, asserted: false, title: None }),
                        WaitErr::Timeout(why, el) => Err(Failed {
                            why: format!("{} 秒内{why}", timeout.as_secs_f64()),
                            marks: el.map(|e| vec![Mark { rect: e.rect, kind: MarkKind::Failure, point: None }]).unwrap_or_default(),
                        }),
                        WaitErr::Fatal(why, rects) => Err(Failed { why, marks: rects.into_iter().map(|r| Mark { rect: r, kind: MarkKind::Failure, point: None }).collect() }),
                    }
                };
                let loc = step.locator().cloned().unwrap_or_default();
                match action {
                    UiAction::Click | UiAction::LongPress | UiAction::Hover => {
                        let el = match wait_element(d, &loc, deadline, true, &cancelled) {
                            Ok(el) => el,
                            Err(e) => return wait_err(e),
                        };
                        let point = el.rect.center();
                        // 操作前拍：标出点了哪里
                        let shot = if all_shots { snap(d, &shot_name(key), &[Mark { rect: el.rect, kind: MarkKind::Action, point: Some(point) }], false).ok() } else { None };
                        let r = match action {
                            UiAction::Click => d.click(&el),
                            UiAction::LongPress => d.long_press(&el),
                            _ => d.hover(&el),
                        };
                        r.map_err(|e| Failed::new(format!("{e:#}")))?;
                        d.settle(timeout);
                        Ok(Done { marks: None, shot, detail: None, skipped: false, asserted: false, title: None })
                    }
                    UiAction::Fill | UiAction::Select => {
                        let el = match wait_element(d, &loc, deadline, true, &cancelled) {
                            Ok(el) => el,
                            Err(e) => return wait_err(e),
                        };
                        let value = step.value.clone().unwrap_or_default();
                        let mark = Mark { rect: el.rect, kind: MarkKind::Action, point: None };
                        let mut title = None;
                        let detail = if action == UiAction::Fill {
                            d.fill(&el, &value).map_err(|e| Failed { why: format!("{e:#}"), marks: vec![mark] })?;
                            if el.secret {
                                title = Some(format!("填写 {} = ••••••", loc.describe()));
                            }
                            None
                        } else {
                            let chosen = d.select(&el, &value).map_err(|e| Failed { why: format!("{e:#}"), marks: vec![mark] })?;
                            Some(format!("选中「{chosen}」"))
                        };
                        d.settle(timeout);
                        Ok(Done { marks: Some(vec![mark]), shot: None, detail, skipped: false, asserted: false, title })
                    }
                    UiAction::WaitFor | UiAction::Expect => {
                        let mut last = String::new();
                        loop {
                            match check_once(d, &loc) {
                                Ok((true, _, rects)) => {
                                    let marks = rects.into_iter().map(|r| Mark { rect: r, kind: MarkKind::Verified, point: None }).collect();
                                    return Ok(Done { marks: Some(marks), shot: None, detail: None, skipped: false, asserted: action == UiAction::Expect, title: None });
                                }
                                Ok((false, why, rects)) => {
                                    last = why;
                                    if Instant::now() >= deadline || cancelled() {
                                        if optional {
                                            return Ok(Done { marks: Some(vec![]), shot: None, detail: Some(format!("跳过：{last}")), skipped: true, asserted: false, title: None });
                                        }
                                        return Err(Failed { why: format!("{} 秒内没有满足：{last}", timeout.as_secs_f64()), marks: rects.into_iter().map(|r| Mark { rect: r, kind: MarkKind::Failure, point: None }).collect() });
                                    }
                                }
                                Err(e) => return Err(Failed::new(format!("{e:#}{}", if last.is_empty() { String::new() } else { format!("（之前：{last}）") }))),
                            }
                            std::thread::sleep(kind.poll());
                        }
                    }
                    UiAction::Goto => {
                        d.goto(step.goto.as_deref().unwrap_or_default()).map_err(|e| Failed::new(format!("{e:#}")))?;
                        d.settle(timeout);
                        Ok(Done { marks: Some(vec![]), shot: None, detail: None, skipped: false, asserted: false, title: None })
                    }
                    UiAction::Wait => {
                        let until = Instant::now() + Duration::from_millis(step.wait.unwrap_or(0));
                        while Instant::now() < until && !cancelled() {
                            std::thread::sleep(until.saturating_duration_since(Instant::now()).min(Duration::from_millis(100)));
                        }
                        Ok(Done { marks: None, shot: None, detail: None, skipped: false, asserted: false, title: None })
                    }
                    UiAction::Screenshot => Ok(Done { marks: Some(vec![]), shot: None, detail: None, skipped: false, asserted: false, title: None }),
                    other => {
                        let r = match other {
                            UiAction::Type => d.type_text(step.type_text.as_deref().unwrap_or_default()),
                            UiAction::Press => d.press(step.press.as_deref().unwrap_or_default()),
                            UiAction::Swipe => d.swipe(step.swipe.as_deref().unwrap_or_default()),
                            UiAction::Back => d.back(),
                            // 桌面应用的 launch 步骤 = 重新启动（先正常退出）
                            UiAction::Launch => d.launch(kind == UiKind::Desktop || step.clear_data == Some(true)),
                            UiAction::Stop => d.stop(),
                            _ => unreachable!(),
                        };
                        r.map_err(|e| Failed::new(format!("{e:#}")))?;
                        d.settle(timeout);
                        Ok(Done { marks: Some(vec![]), shot: None, detail: None, skipped: false, asserted: false, title: None })
                    }
                }
            })();

            // ---- 屏幕文字识别的摘要：通过时记命中与置信度，失败时列出屏幕上识别到的文字 ----
            let res = if step_ocr {
                let note = crate::engine::ocr_match::take_note();
                match (res, note) {
                    (Ok(mut done), Some(nt)) => {
                        done.detail = Some(match done.detail {
                            Some(d) => format!("{d}；{nt}"),
                            None => nt,
                        });
                        Ok(done)
                    }
                    (Err(mut f), Some(nt)) => {
                        f.why = format!("{}（{nt}）", f.why);
                        Err(f)
                    }
                    (res, None) => res,
                }
            } else {
                res
            };

            // ---- 捕获 ----
            let res = match res {
                Ok(mut done) if !done.skipped && !step.capture.is_empty() => match self.ui_capture(d, &step, deadline, &cancelled) {
                    Ok(got) => {
                        let text = got.iter().map(|(k, v)| format!("{k} = {}", super::clip_preview_short(v))).collect::<Vec<_>>().join("，");
                        for (k, val) in got {
                            self.runtime_vars.insert(k, VarVal::Str(val));
                        }
                        done.detail = Some(match done.detail {
                            Some(d) => format!("{d}；捕获 {text}"),
                            None => format!("捕获 {text}"),
                        });
                        Ok(done)
                    }
                    Err(f) => Err(f),
                },
                other => other,
            };

            let mut title = title;
            if let Ok(Done { title: Some(t), .. }) = &res {
                title = t.clone();
            }
            let mut notes = d.take_notes();
            let duration = started.elapsed().as_millis() as u64 - at;
            let (status, shot, detail) = match res {
                Ok(done) => {
                    let shot = match (done.shot, done.marks) {
                        (Some(s), _) => Some(s),
                        (None, Some(marks)) if all_shots || step.screenshot.is_some() => {
                            let name = match &step.screenshot {
                                Some(label) => format!("{n:02}-{}.jpg", { let s = file_slug(label); if s.is_empty() { "screenshot".to_string() } else { s } }),
                                None => shot_name(key),
                            };
                            match snap(d, &name, &marks, observe_only) {
                                Ok(s) => Some(s),
                                Err(e) => {
                                    notes.push(e);
                                    None
                                }
                            }
                        }
                        _ => None,
                    };
                    if done.asserted {
                        outcomes.push(TestOutcome { expr: format!("#{n} {title}"), pass: true, detail: None });
                    }
                    (if done.skipped { UiStepStatus::Skipped } else { UiStepStatus::Passed }, shot, done.detail)
                }
                Err(f) => {
                    let shot = snap(d, &shot_name(&format!("{key}-failed")), &f.marks, observe_only).ok();
                    outcomes.push(TestOutcome { expr: format!("#{n} {title}"), pass: false, detail: Some(f.why.clone()) });
                    failed = true;
                    ocr_failed = step_ocr;
                    (UiStepStatus::Failed, shot, Some(f.why))
                }
            };
            let detail = match (detail, notes.is_empty()) {
                (d, true) => d,
                (Some(d), false) => Some(format!("{d}；{}", notes.join("；"))),
                (None, false) => Some(notes.join("；")),
            };
            let mark = match status {
                UiStepStatus::Passed => "✔",
                UiStepStatus::Failed => "✘",
                UiStepStatus::Skipped => "⇣",
            };
            self.emit(RunEvent::Log { file: rel.clone(), line: self.redact_str(&format!("{mark} #{n} {title}  {duration}ms")) });
            let rec = StepRecord { index: n, action: key.to_string(), title, status, at_ms: at, duration_ms: duration, screenshot: shot, detail };
            self.emit_ui_step(&rel, &ev.dir, &rec);
            ev.steps.push(rec);
            if failed {
                break;
            }
        }

        // ---- 结束画面 + 失败现场 ----
        let n = steps.len() + 1;
        let at = started.elapsed().as_millis() as u64;
        if !failed {
            let shot = snap(d, &format!("{n:02}-final.jpg"), &[], false).ok();
            let rec = StepRecord { index: n, action: "final".into(), title: "结束画面".into(), status: UiStepStatus::Passed, at_ms: at, duration_ms: 0, screenshot: shot, detail: None };
            self.emit_ui_step(&rel, &ev.dir, &rec);
            ev.steps.push(rec);
        } else {
            ev.artifacts = d.failure_scene(dir);
            // 屏幕文字识别的失败现场：识别到的全部文字、位置与置信度（对照着改定位）
            if ocr_failed {
                if let Some(frame) = crate::engine::ocr::take_last() {
                    let text = serde_json::to_string_pretty(&frame.to_json()).unwrap_or_default();
                    if crate::fsx::write(dir.join("ocr.json"), text.as_bytes()).is_ok() {
                        ev.artifacts.push(eui::Artifact { label: "屏幕文字识别（OCR）".into(), file: "ocr.json".into() });
                    }
                }
            }
        }
        (outcomes, ev)
    }

    /// 本步完成后的捕获：(变量名, 值)
    fn ui_capture(&mut self, d: &mut dyn UiDriver, step: &UiStep, deadline: Instant, cancelled: &dyn Fn() -> bool) -> Result<Vec<(String, String)>, Failed> {
        let mut got = vec![];
        for (name, src) in &step.capture {
            let raw = match src {
                CaptureSource::Builtin(b) => {
                    let p = d.page().map_err(|e| Failed::new(format!("捕获 {name}：{e:#}")))?;
                    match b.as_str() {
                        "url" | "package" | "app" => p.location,
                        "title" | "activity" | "window" => p.title,
                        other => return Err(Failed::new(format!("捕获 {name}：不认识的来源 {other}（Web 写 url / title，Android 写 activity，Mac 桌面写 window，或写一个元素定位）"))),
                    }
                }
                CaptureSource::Element(loc) => {
                    let el = wait_element(d, loc, deadline, false, cancelled).map_err(|e| match e {
                        WaitErr::Timeout(w, _) | WaitErr::Fatal(w, _) => Failed::new(format!("捕获 {name}：{w}")),
                    })?;
                    match &loc.attr {
                        Some(a) => d.attr(&el, a).map_err(|e| Failed::new(format!("捕获 {name}：{e:#}")))?.ok_or_else(|| Failed::new(format!("捕获 {name}：元素没有属性 {a}")))?,
                        None => el.value.clone().filter(|v| !v.is_empty()).unwrap_or(el.text.clone()),
                    }
                }
            };
            let regex = match src {
                CaptureSource::Element(l) => l.regex.clone(),
                _ => None,
            };
            let val = match regex {
                Some(re) => {
                    let re = regex::Regex::new(&re).map_err(|e| Failed::new(format!("捕获 {name}：正则写法有误：{e}")))?;
                    let caps = re.captures(&raw).ok_or_else(|| Failed::new(format!("捕获 {name}：「{}」里没有匹配正则的内容", super::clip_preview_short(&raw))))?;
                    caps.get(1).or(caps.get(0)).map(|m| m.as_str().to_string()).unwrap_or_default()
                }
                None => raw,
            };
            got.push((name.clone(), val));
        }
        Ok(got)
    }
}

/// 步骤里的文字替换变量
fn render_step(s: &UiStep, v: &Vars) -> UiStep {
    let r = |o: &Option<String>| o.as_ref().map(|x| v.render(x));
    let rl = |o: &Option<Locator>| o.as_ref().map(|l| render_locator(l, v));
    UiStep {
        goto: r(&s.goto),
        click: rl(&s.click),
        tap: rl(&s.tap),
        long_press: rl(&s.long_press),
        hover: rl(&s.hover),
        fill: rl(&s.fill),
        select: rl(&s.select),
        type_text: r(&s.type_text),
        press: r(&s.press),
        swipe: r(&s.swipe),
        wait_for: rl(&s.wait_for),
        expect: rl(&s.expect),
        screenshot: r(&s.screenshot),
        value: r(&s.value),
        capture: s
            .capture
            .iter()
            .map(|(k, c)| {
                (
                    k.clone(),
                    match c {
                        CaptureSource::Element(l) => CaptureSource::Element(Box::new(render_locator(l, v))),
                        other => other.clone(),
                    },
                )
            })
            .collect(),
        ..s.clone()
    }
}

/// 步骤清单（dry-run 预览）
pub fn describe_steps(steps: &[UiStep], vars: &Vars, kind: UiKind) -> String {
    steps.iter().enumerate().map(|(i, s)| format!("#{} {}", i + 1, describe_ui_step(&render_step(s, vars), kind))).collect::<Vec<_>>().join("\n")
}
