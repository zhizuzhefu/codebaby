//! Mac 桌面驱动：agent-desktop（macOS 无障碍树），不需要在被测应用里装任何东西。
//!
//! - 界面结构：`agent-desktop snapshot --include-bounds`（和系统无障碍看到的一致：角色、名称、值、状态、可用动作）；
//!   定位在本地对整棵树匹配，写法与 Android 一致（`role` + `name` / `text` / `id` / `desc`）
//! - 操作：`click` / `set-value` / `type` / `select` / `scroll` / `press`，默认走无障碍语义调用，
//!   不抢焦点、不动鼠标；没有语义动作的元素会明确报错，不会悄悄改用真实鼠标
//! - 证据：`screenshot` 截窗口（去掉阴影），失败时留无障碍树文本
//!
//! 需要运行 requestbaby 的程序（终端 / requestbaby.app）在「系统设置 → 隐私与安全性」里有辅助功能权限，
//! 截图还需要屏幕录制权限（`requestbaby doctor` 会检查）。
use super::ui::{Actionable, Artifact, Element, PageInfo, Rect, Shot, UiDriver};
use crate::config::{Locator, UiKind};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// 读界面结构的预算：大应用（Electron / Xcode）可能很慢，超出会得到不完整的树
const SNAPSHOT_MS: u64 = 8_000;
/// 一条 agent-desktop 命令的整体上限
const CALL: Duration = Duration::from_secs(40);

// ---------------------------------------------------------------------------------------------
// 找到 agent-desktop，调用并解析它的 JSON 信封
// ---------------------------------------------------------------------------------------------

/// agent-desktop 可执行文件：项目配置 → REQUESTBABY_AGENT_DESKTOP → PATH → 常见安装位置 → 登录 shell 的 PATH。
/// 从 Finder 启动的 App 拿不到终端里的 PATH（npm 全局目录在 nvm 下），所以多找几个地方
pub fn find_agent_desktop(configured: Option<&str>) -> Result<PathBuf> {
    let named = |p: &str| -> Option<PathBuf> {
        let pb = PathBuf::from(p);
        if pb.is_file() { Some(pb) } else { super::cdp::which(p) }
    };
    if let Some(p) = std::env::var("REQUESTBABY_AGENT_DESKTOP").ok().filter(|s| !s.is_empty()) {
        return named(&p).ok_or_else(|| anyhow!("找不到 agent-desktop：{p}（环境变量 REQUESTBABY_AGENT_DESKTOP）"));
    }
    if let Some(p) = configured.filter(|s| !s.is_empty()) {
        return named(p).ok_or_else(|| anyhow!("找不到 agent-desktop：{p}（requestbaby.json 的 desktop.agentDesktop）"));
    }
    if let Some(p) = super::cdp::which("agent-desktop") {
        return Ok(p);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut dirs: Vec<PathBuf> = vec!["/opt/homebrew/bin".into(), "/usr/local/bin".into()];
    if let Some(h) = &home {
        dirs.extend([h.join(".npm-global/bin"), h.join(".local/bin"), h.join(".cargo/bin"), h.join(".bun/bin"), h.join(".volta/bin")]);
        if let Ok(rd) = std::fs::read_dir(h.join(".nvm/versions/node")) {
            dirs.extend(rd.flatten().map(|e| e.path().join("bin")));
        }
    }
    if let Some(p) = dirs.into_iter().map(|d| d.join("agent-desktop")).find(|p| p.is_file()) {
        return Ok(p);
    }
    // 最后问一次登录 shell：用户在 .zshrc 里配的 PATH
    if let Some(shell) = std::env::var("SHELL").ok().filter(|s| !s.is_empty()) {
        if let Ok(out) = Command::new(shell).args(["-lic", "command -v agent-desktop"]).stdin(Stdio::null()).stderr(Stdio::null()).output() {
            let found = String::from_utf8_lossy(&out.stdout).lines().last().unwrap_or_default().trim().to_string();
            if !found.is_empty() && Path::new(&found).is_file() {
                return Ok(PathBuf::from(found));
            }
        }
    }
    bail!("没有找到 agent-desktop：运行 `npm install -g agent-desktop` 安装，或在 requestbaby.json 的 desktop.agentDesktop 写路径")
}

/// agent-desktop 返回的错误（信封里 ok = false）
#[derive(Debug, Clone)]
pub struct AdError {
    pub code: String,
    pub message: String,
    pub suggestion: String,
    /// 动作是否可能已经生效（delivery 不是 not_delivered）
    pub maybe_delivered: bool,
}

impl std::fmt::Display for AdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}（{}）", self.message, self.code)?;
        let hint = match self.code.as_str() {
            "PERM_DENIED" => "：给运行 requestbaby 的程序开启辅助功能权限（agent-desktop permissions --guide），截图还需要屏幕录制权限".to_string(),
            "APP_NOT_FOUND" => "：应用没有在运行（用例开头会自动启动，或检查 app 写的名称 / bundle ID）".to_string(),
            "WINDOW_NOT_FOUND" => "：应用没有可见窗口（先等它打开窗口，或检查 window 写的标题）".to_string(),
            _ if !self.suggestion.is_empty() => format!("：{}", self.suggestion),
            _ => String::new(),
        };
        write!(f, "{hint}")
    }
}

impl std::error::Error for AdError {}

/// 一次命令调用：运行 → 解析信封 → 返回 data
#[derive(Debug, Clone)]
pub struct AgentDesktop {
    pub path: PathBuf,
}

impl AgentDesktop {
    pub fn from_config(cfg: &crate::config::ui::DesktopConfig) -> Result<Self> {
        Ok(Self { path: find_agent_desktop(cfg.agent_desktop.as_deref())? })
    }

    /// 运行并返回 `data`；信封 ok = false 时返回 [`AdError`]
    pub fn call(&self, args: &[&str], timeout: Duration) -> Result<Value> {
        let mut child = Command::new(&self.path).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().with_context(|| format!("运行 agent-desktop 失败：{}", self.path.display()))?;
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let out = std::thread::spawn(move || {
            let mut b = vec![];
            let _ = stdout.read_to_end(&mut b);
            b
        });
        let err = std::thread::spawn(move || {
            let mut b = vec![];
            let _ = stderr.read_to_end(&mut b);
            b
        });
        let deadline = Instant::now() + timeout;
        loop {
            if child.try_wait()?.is_some() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                bail!("agent-desktop {} 超过 {} 秒没有结束（应用可能无响应）", args.first().copied().unwrap_or_default(), timeout.as_secs());
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        let out = out.join().unwrap_or_default();
        let err = String::from_utf8_lossy(&err.join().unwrap_or_default()).trim().to_string();
        parse_envelope(&out).map_err(|e| if err.is_empty() { e } else { anyhow!("{e:#}（{}）", err.lines().last().unwrap_or_default()) })
    }
}

/// 解析信封 `{"ok": true, "data": …}` / `{"ok": false, "error": {code, message, …}}`
pub fn parse_envelope(bytes: &[u8]) -> Result<Value> {
    let v: Value = serde_json::from_slice(bytes).map_err(|_| anyhow!("agent-desktop 没有返回 JSON：{}", String::from_utf8_lossy(bytes).trim().chars().take(200).collect::<String>()))?;
    if v.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(v.get("data").cloned().unwrap_or(Value::Null));
    }
    let e = v.get("error").cloned().unwrap_or(Value::Null);
    let s = |k: &str| e.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let delivery = e.pointer("/disposition/delivery").and_then(Value::as_str).unwrap_or("not_delivered");
    Err(AdError { code: s("code"), message: s("message"), suggestion: s("suggestion"), maybe_delivered: delivery != "not_delivered" }.into())
}

// ---------------------------------------------------------------------------------------------
// 界面结构
// ---------------------------------------------------------------------------------------------

/// 界面结构里的一个节点
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Node {
    pub role: String,
    pub name: String,
    pub value: Option<String>,
    pub description: String,
    pub placeholder: String,
    /// AXIdentifier
    pub native_id: String,
    pub ref_id: Option<String>,
    /// 自己或最近的有引用的祖先（静态文字没有引用，点它实际是点它所在的行 / 按钮）
    pub act_ref: Option<String>,
    pub actions: Vec<String>,
    pub states: Vec<String>,
    /// 相对窗口左上角
    pub rect: Rect,
    pub depth: usize,
}

impl Node {
    pub fn has_state(&self, s: &str) -> bool {
        self.states.iter().any(|x| x == s)
    }

    pub fn can(&self, action: &str) -> bool {
        self.actions.iter().any(|x| x == action)
    }

    fn checkable(&self) -> bool {
        matches!(self.role.as_str(), "checkbox" | "radiobutton" | "switch") || self.can("Toggle")
    }

    fn secret(&self) -> bool {
        self.role.contains("secure") || self.role.contains("password")
    }

    fn text_input(&self) -> bool {
        matches!(self.role.as_str(), "textfield" | "textarea" | "searchfield" | "combobox") || self.secret()
    }
}

/// 一次界面快照
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub id: String,
    pub app: String,
    pub window_id: String,
    pub window_title: String,
    /// 窗口在屏幕上的位置（节点坐标已换成相对它的）
    pub window: Rect,
    /// 树没读完（超出读取预算）
    pub partial: bool,
    pub nodes: Vec<Node>,
}

fn rect_of(v: &Value) -> Rect {
    let n = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    Rect { x: n("x"), y: n("y"), w: n("width"), h: n("height") }
}

/// 解析 `snapshot --include-bounds` 的 data
pub fn parse_snapshot(data: &Value) -> Result<Snapshot> {
    let tree = data.get("tree").ok_or_else(|| anyhow!("界面结构为空"))?;
    let window = tree.get("bounds").map(rect_of).unwrap_or_default();
    let mut snap = Snapshot {
        id: data.get("snapshot_id").and_then(Value::as_str).unwrap_or_default().to_string(),
        app: data.get("app").and_then(Value::as_str).unwrap_or_default().to_string(),
        window_id: data.pointer("/window/id").and_then(Value::as_str).unwrap_or_default().to_string(),
        window_title: data.pointer("/window/title").and_then(Value::as_str).unwrap_or_default().to_string(),
        window,
        partial: data.get("complete").and_then(Value::as_bool) == Some(false),
        nodes: vec![],
    };
    walk(tree, 0, None, &window, &mut snap.nodes);
    Ok(snap)
}

fn walk(v: &Value, depth: usize, parent_ref: Option<&str>, win: &Rect, out: &mut Vec<Node>) {
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let list = |k: &str| v.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    let mut rect = v.get("bounds").map(rect_of).unwrap_or_default();
    rect.x -= win.x;
    rect.y -= win.y;
    let ref_id = v.get("ref_id").and_then(Value::as_str).map(String::from);
    let node = Node {
        role: s("role").to_ascii_lowercase(),
        name: s("name"),
        value: v.get("value").and_then(|x| match x {
            Value::String(t) => Some(t.clone()),
            Value::Null => None,
            other => Some(other.to_string()),
        }),
        description: s("description"),
        placeholder: s("placeholder"),
        native_id: v.pointer("/native_id/value").and_then(Value::as_str).unwrap_or_default().to_string(),
        act_ref: ref_id.clone().or_else(|| parent_ref.map(String::from)),
        ref_id: ref_id.clone(),
        actions: list("available_actions"),
        states: list("states"),
        rect,
        depth,
    };
    let own = node.act_ref.clone();
    out.push(node);
    for c in v.get("children").and_then(Value::as_array).into_iter().flatten() {
        walk(c, depth + 1, own.as_deref(), win, out);
    }
}

/// 角色匹配：写 textbox / radio / img 等通用名字也认
fn role_matches(actual: &str, want: &str) -> bool {
    let canon = |r: &str| match r.to_ascii_lowercase().as_str() {
        "textbox" | "textarea" | "searchbox" | "searchfield" | "input" => "textfield".to_string(),
        "radio" => "radiobutton".to_string(),
        "img" => "image".to_string(),
        "text" | "label" => "statictext".to_string(),
        "gridcell" => "cell".to_string(),
        other => other.to_string(),
    };
    canon(actual) == canon(want)
}

/// 节点是否符合定位
pub fn node_matches(n: &Node, loc: &Locator) -> bool {
    let exact = loc.exact.unwrap_or(false);
    let m = |actual: &str, want: &str| super::ui::text_matches(actual, want, exact);
    let value = n.value.as_deref().unwrap_or_default();
    if let Some(id) = &loc.id {
        if !(n.native_id == *id || (!n.native_id.is_empty() && n.native_id.ends_with(id.as_str()))) {
            return false;
        }
    }
    if let Some(t) = &loc.text {
        if !(m(&n.name, t) || m(value, t) || m(&n.description, t)) {
            return false;
        }
    }
    if let Some(d) = &loc.desc {
        if !(m(&n.description, d) || m(&n.name, d)) {
            return false;
        }
    }
    if let Some(l) = &loc.label {
        // 表单标签：输入框的名称 / 描述 / 占位文字
        if !(n.text_input() && (m(&n.name, l) || m(&n.description, l) || m(&n.placeholder, l))) {
            return false;
        }
    }
    if let Some(p) = &loc.placeholder {
        if !m(&n.placeholder, p) {
            return false;
        }
    }
    if let Some(role) = &loc.role {
        if !role_matches(&n.role, role) {
            return false;
        }
        if let Some(name) = &loc.name {
            if !(m(&n.name, name) || m(&n.description, name) || (n.role == "statictext" && m(value, name))) {
                return false;
            }
        }
    }
    loc.has_target()
}

/// 无障碍树的文本大纲（失败现场）
pub fn outline(snap: &Snapshot) -> String {
    let mut out = String::new();
    if snap.partial {
        out.push_str("（读取预算用尽，树不完整）\n");
    }
    for n in &snap.nodes {
        let mut line = format!("{}{}", "  ".repeat(n.depth), n.role);
        if !n.name.is_empty() {
            line.push_str(&format!(" \"{}\"", n.name.replace('\n', " ")));
        }
        if let Some(v) = n.value.as_deref().filter(|v| !v.is_empty() && *v != n.name) {
            let v = if n.secret() { "••••••".to_string() } else { v.replace('\n', " ").chars().take(120).collect() };
            line.push_str(&format!(" = {v}"));
        }
        if !n.states.is_empty() {
            line.push_str(&format!(" [{}]", n.states.join(",")));
        }
        if !n.native_id.is_empty() {
            line.push_str(&format!(" #{}", n.native_id));
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// 按键名 → agent-desktop 的组合键：Enter → return，Control → ctrl，Meta / Command → cmd，ArrowDown → down …
pub fn combo(key: &str) -> Result<String> {
    let k = key.trim();
    if k.is_empty() {
        bail!("press 要写按键（Enter / Tab / Escape / Meta+s …）");
    }
    let mut parts: Vec<String> = vec![];
    let pieces: Vec<&str> = if k == "+" { vec!["+"] } else { k.split('+').collect() };
    for p in pieces {
        let low = p.trim().to_ascii_lowercase();
        parts.push(match low.as_str() {
            "enter" | "return" => "return".into(),
            "esc" | "escape" => "escape".into(),
            "backspace" | "delete" => "delete".into(),
            "control" | "ctrl" => "ctrl".into(),
            "meta" | "command" | "cmd" | "super" => "cmd".into(),
            "alt" | "option" | "opt" => "alt".into(),
            "shift" => "shift".into(),
            "arrowup" | "up" => "up".into(),
            "arrowdown" | "down" => "down".into(),
            "arrowleft" | "left" => "left".into(),
            "arrowright" | "right" => "right".into(),
            "" => bail!("不认识的按键 {key}"),
            _ if p.trim().chars().count() == 1 => p.trim().to_string(),
            other => other.to_string(),
        });
    }
    Ok(parts.join("+"))
}

// ---------------------------------------------------------------------------------------------
// 驱动
// ---------------------------------------------------------------------------------------------

/// 一个桌面应用（及其窗口）
pub struct App {
    ad: AgentDesktop,
    /// 用例里写的应用名称或 bundle ID
    pub app: String,
    /// 命令行里用的显示名称（启动 / 解析后确定）
    name: String,
    /// 只看标题包含它的窗口
    window_filter: Option<String>,
    /// 已锁定的窗口 ID（window 写了标题时）
    window_id: Option<String>,
    launch_timeout: Duration,
    snap: Snapshot,
    scale: f64,
    notes: Vec<String>,
}

impl App {
    pub fn new(ad: AgentDesktop, app: String, window_filter: Option<String>, launch_timeout: Duration) -> Self {
        Self { ad, name: app.clone(), app, window_filter, window_id: None, launch_timeout, snap: Snapshot::default(), scale: 2.0, notes: vec![] }
    }

    /// 接管一个已在运行的应用（不启动、不退出它）：snapshot 之类只读场景用
    pub fn attach(&mut self) -> Result<()> {
        let (name, _) = self.running().ok_or_else(|| anyhow!("应用「{}」没有在运行：先打开它（app 写应用名称或 bundle ID）", self.app))?;
        self.name = name;
        self.lock_window()
    }

    /// 当前的界面快照节点
    pub fn nodes(&mut self) -> Result<Vec<Node>> {
        self.refresh()?;
        Ok(self.snap.nodes.clone())
    }

    pub fn snapshot(&mut self) -> Result<&Snapshot> {
        self.refresh()?;
        Ok(&self.snap)
    }

    fn call(&self, args: &[&str]) -> Result<Value> {
        self.ad.call(args, CALL)
    }

    /// 应用目标参数：`--app 名称`（锁定了窗口时加 `--window-id`）
    fn target(&self) -> Vec<String> {
        let mut t = vec!["--app".to_string(), self.name.clone()];
        if let Some(w) = &self.window_id {
            t.push("--window-id".into());
            t.push(w.clone());
        }
        t
    }

    fn call_with(&self, head: &[&str], tail: &[String]) -> Result<Value> {
        let mut args: Vec<&str> = head.to_vec();
        args.extend(tail.iter().map(|s| s.as_str()));
        self.call(&args)
    }

    /// 正在运行的应用：(显示名称, bundle ID)。app 可以写名称或 bundle ID
    fn running(&self) -> Option<(String, String)> {
        let data = self.call(&["list-apps"]).ok()?;
        let want = self.app.to_lowercase();
        data.get("apps")?.as_array()?.iter().find_map(|a| {
            let name = a.get("name")?.as_str()?.to_string();
            let bundle = a.get("bundle_id").and_then(Value::as_str).unwrap_or_default().to_string();
            (name.to_lowercase() == want || bundle.to_lowercase() == want).then_some((name, bundle))
        })
    }

    /// window 写了标题时：找出标题包含它的窗口并锁定
    fn lock_window(&mut self) -> Result<()> {
        let Some(filter) = self.window_filter.clone() else { return Ok(()) };
        let data = self.call(&["list-windows", "--app", &self.name])?;
        let wins = data.as_array().cloned().unwrap_or_default();
        let hits: Vec<&Value> = wins.iter().filter(|w| w.get("title").and_then(Value::as_str).unwrap_or_default().to_lowercase().contains(&filter.to_lowercase())).collect();
        match hits.as_slice() {
            [one] => {
                self.window_id = one.get("id").and_then(Value::as_str).map(String::from);
                Ok(())
            }
            [] => {
                let titles: Vec<String> = wins.iter().filter_map(|w| w.get("title").and_then(Value::as_str)).map(|t| format!("「{t}」")).collect();
                bail!("{} 没有标题包含「{filter}」的窗口（现有：{}）", self.name, if titles.is_empty() { "无".to_string() } else { titles.join("、") })
            }
            many => bail!("{} 有 {} 个标题包含「{filter}」的窗口：把 window 写得更完整", self.name, many.len()),
        }
    }

    /// 应用有多个窗口（主窗口 + 保存对话框 / 弹出的面板）又没写 window 时：操作获得焦点的那个，没有焦点的取第一个可见的
    fn front_window(&self) -> Result<String> {
        let data = self.call(&["list-windows", "--app", &self.name])?;
        let wins = data.as_array().cloned().unwrap_or_default();
        let visible = |w: &&Value| w.get("visible").and_then(Value::as_bool) != Some(false) && w.get("minimized").and_then(Value::as_bool) != Some(true);
        let pick = wins.iter().filter(visible).find(|w| w.get("is_focused").and_then(Value::as_bool) == Some(true)).or_else(|| wins.iter().find(visible));
        pick.and_then(|w| w.get("id").and_then(Value::as_str)).map(String::from).ok_or_else(|| anyhow!("{} 没有可见的窗口", self.name))
    }

    fn snapshot_call(&self, window: Option<&str>) -> Result<Value> {
        let timeout = SNAPSHOT_MS.to_string();
        let mut args = vec!["snapshot".to_string(), "--include-bounds".into(), "--max-depth".into(), "20".into(), "--timeout-ms".into(), timeout];
        match window {
            Some(w) => args.extend(["--app".to_string(), self.name.clone(), "--window-id".into(), w.to_string()]),
            None => args.extend(self.target()),
        }
        self.ad.call(&args.iter().map(|s| s.as_str()).collect::<Vec<_>>(), Duration::from_millis(SNAPSHOT_MS + 15_000))
    }

    fn refresh(&mut self) -> Result<()> {
        self.lock_window()?;
        let data = match self.snapshot_call(None) {
            Err(e) if self.window_id.is_none() && matches!(e.downcast_ref::<AdError>(), Some(a) if a.code == "AMBIGUOUS_TARGET") => {
                let w = self.front_window()?;
                self.snapshot_call(Some(&w))?
            }
            other => other?,
        };
        self.snap = parse_snapshot(&data)?;
        if self.snap.partial {
            let note = "界面很大，在读取预算内没读完，可能找不到靠后的元素".to_string();
            if !self.notes.contains(&note) {
                self.notes.push(note);
            }
        }
        Ok(())
    }

    fn element_of(&self, i: usize) -> Element {
        let n = &self.snap.nodes[i];
        let visible = !n.has_state("hidden") && !n.has_state("offscreen") && !n.rect.is_empty();
        let valued = !matches!(n.role.as_str(), "checkbox" | "radiobutton" | "switch" | "scrollbar" | "statictext" | "menubutton");
        let value = n.value.clone().filter(|_| valued);
        let text = if n.text_input() {
            n.value.clone().unwrap_or_default()
        } else if n.role == "statictext" {
            n.value.clone().filter(|v| !v.is_empty()).unwrap_or_else(|| n.name.clone())
        } else if !n.name.is_empty() {
            n.name.clone()
        } else {
            n.value.clone().filter(|_| valued).filter(|v| !v.is_empty()).unwrap_or_else(|| n.description.clone())
        };
        Element {
            handle: i,
            rect: n.rect,
            visible,
            enabled: !n.has_state("disabled"),
            checked: n.checkable().then(|| n.has_state("checked")),
            editable: n.text_input() && !n.has_state("readonly") && (n.can("SetValue") || n.can("TypeText")),
            readonly: n.has_state("readonly"),
            text,
            value: if n.secret() { None } else { value },
            secret: n.secret(),
            tag: format!("{}{}", n.role, if n.name.is_empty() { String::new() } else { format!(" \"{}\"", n.name.chars().take(30).collect::<String>()) }),
        }
    }

    /// 元素当前可用来操作的引用；快照过期（STALE_REF / AMBIGUOUS_TARGET）时重新读界面，按角色 + 名称 + 标识找回同一个元素再试一次
    fn with_ref(&mut self, el: &Element, op: &dyn Fn(&AgentDesktop, &str) -> Result<Value>) -> Result<()> {
        let node = self.snap.nodes.get(el.handle).cloned().ok_or_else(|| anyhow!("元素已失效，请重新查找"))?;
        let r = node.act_ref.clone().ok_or_else(|| anyhow!("{} 没有可操作的无障碍引用（只有真实鼠标才能点它，暂不支持）", el.tag))?;
        match op(&self.ad, &r) {
            Ok(_) => Ok(()),
            Err(e) if matches!(e.downcast_ref::<AdError>(), Some(a) if matches!(a.code.as_str(), "STALE_REF" | "AMBIGUOUS_TARGET" | "SNAPSHOT_NOT_FOUND") && !a.maybe_delivered) => {
                self.refresh()?;
                let same = self.snap.nodes.iter().filter(|n| n.role == node.role && n.name == node.name && n.native_id == node.native_id && n.act_ref.is_some()).min_by(|a, b| {
                    let d = |n: &&Node| (n.rect.x - node.rect.x).abs() + (n.rect.y - node.rect.y).abs();
                    d(a).partial_cmp(&d(b)).unwrap_or(std::cmp::Ordering::Equal)
                });
                let r2 = same.and_then(|n| n.act_ref.clone()).ok_or_else(|| anyhow!("{} 在操作前消失了（界面已变化）", el.tag))?;
                op(&self.ad, &r2).map(|_| ())
            }
            Err(e) => Err(e),
        }
    }

    /// 正常退出应用（不强杀，避免丢用户数据）：应用在等确认保存时报清楚原因
    fn quit(&self) -> Result<()> {
        self.call(&["close-app", &self.name.clone()]).map(|_| ()).map_err(|e| match e.downcast_ref::<AdError>() {
            Some(a) if a.code == "TIMEOUT" => anyhow!("{} 没有退出：可能在等你确认保存 / 关闭（正常退出不会强杀，避免丢数据）：先在应用里处理，或不要 restart / stop", self.name),
            _ => e,
        })
    }

    /// 当前焦点所在的可输入元素
    fn focused_input(&mut self) -> Result<Node> {
        self.refresh()?;
        self.snap
            .nodes
            .iter()
            .find(|n| n.has_state("focused") && n.act_ref.is_some() && (n.can("TypeText") || n.can("SetValue")))
            .cloned()
            .ok_or_else(|| anyhow!("当前焦点不在输入框里：先 click 输入框，或改用 fill"))
    }

    fn screenshot_png(&self) -> Result<(Vec<u8>, f64)> {
        let tmp = std::env::temp_dir().join(format!("requestbaby-desktop-{}-{}.png", std::process::id(), crate::history::now_ms()));
        let path = tmp.to_string_lossy().to_string();
        // 截的是最近一次读界面时操作的那个窗口（多窗口时目标应用名不足以确定窗口）
        let target = if self.snap.window_id.is_empty() { self.target() } else { vec!["--app".into(), self.name.clone(), "--window-id".into(), self.snap.window_id.clone()] };
        let r = self.call_with(&["screenshot"], &[target, vec![path.clone()]].concat());
        let bytes = std::fs::read(&tmp);
        let _ = std::fs::remove_file(&tmp);
        let data = r?;
        let scale = data.get("scale_factor").and_then(Value::as_f64).unwrap_or(2.0);
        Ok((bytes.context("读取截图失败（屏幕录制权限没开时会这样）")?, scale))
    }
}

/// 窗口截图自带半透明阴影：按「完全不透明」的范围裁出窗口本身，这样元素坐标（相对窗口左上角）能直接对上图片
pub fn crop_to_window(png: &[u8], window: &Rect, scale: f64) -> Option<Vec<u8>> {
    let img = image::load_from_memory(png).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0u32, 0u32);
    for (x, y, p) in img.enumerate_pixels() {
        if p.0[3] >= 250 {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let (cw, ch) = (x1 - x0 + 1, y1 - y0 + 1);
    // 范围和窗口尺寸对不上（没有阴影 / 不是整窗截图）：不裁
    if window.is_empty() || ((cw as f64) - window.w * scale).abs() > 4.0 || ((ch as f64) - window.h * scale).abs() > 4.0 {
        return None;
    }
    let cropped = image::imageops::crop_imm(&img, x0, y0, cw, ch).to_image();
    let mut out = vec![];
    image::DynamicImage::ImageRgba8(cropped).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).ok()?;
    Some(out)
}

impl UiDriver for App {
    fn kind(&self) -> UiKind {
        UiKind::Desktop
    }

    fn find(&mut self, loc: &Locator) -> Result<Vec<Element>> {
        if loc.css.is_some() || loc.test_id.is_some() {
            bail!("Mac 桌面用 role + name / text / label / id / desc 定位（css / testId 只用于 Web）");
        }
        match self.refresh() {
            Ok(()) => {}
            // 窗口正在打开 / 关闭的瞬间：当作暂时没找到，让步骤继续等到超时
            Err(e) if matches!(e.downcast_ref::<AdError>(), Some(a) if a.code == "WINDOW_NOT_FOUND") => {
                self.snap = Snapshot { app: self.name.clone(), ..Default::default() };
                let note = format!("{} 当前没有可见窗口", self.name);
                if !self.notes.contains(&note) {
                    self.notes.push(note);
                }
                return Ok(vec![]);
            }
            Err(e) => return Err(e),
        }
        Ok((0..self.snap.nodes.len()).filter(|i| node_matches(&self.snap.nodes[*i], loc)).map(|i| self.element_of(i)).collect())
    }

    fn page(&mut self) -> Result<PageInfo> {
        self.lock_window()?;
        let data = self.call(&["list-windows", "--app", &self.name])?;
        let wins = data.as_array().cloned().unwrap_or_default();
        let pick = match &self.window_id {
            Some(id) => wins.iter().find(|w| w.get("id").and_then(Value::as_str) == Some(id.as_str())),
            None => wins.iter().find(|w| w.get("is_focused").and_then(Value::as_bool) == Some(true)).or(wins.first()),
        };
        Ok(PageInfo { location: self.name.clone(), title: pick.and_then(|w| w.get("title").and_then(Value::as_str)).unwrap_or_default().to_string() })
    }

    fn actionable(&mut self, el: &Element) -> Result<Actionable> {
        let Some(n) = self.snap.nodes.get(el.handle) else { bail!("元素已失效，请重新查找") };
        if n.act_ref.is_none() {
            bail!("{} 没有可操作的无障碍引用（只有真实鼠标才能点它，暂不支持）", el.tag);
        }
        if n.has_state("offscreen") {
            return Ok(Actionable::NotYet("不在窗口可见范围内（先 swipe 滚动到它）".into()));
        }
        Ok(Actionable::Ok(el.clone()))
    }

    fn click(&mut self, el: &Element) -> Result<()> {
        self.with_ref(el, &|ad, r| ad.call(&["click", r], CALL))
    }

    fn long_press(&mut self, _el: &Element) -> Result<()> {
        bail!("Mac 桌面用例不支持 longPress（Android 专用）")
    }

    fn hover(&mut self, _el: &Element) -> Result<()> {
        bail!("Mac 桌面用例暂不支持 hover：悬停需要真实鼠标，requestbaby 默认只走无障碍语义调用，不会动你的鼠标")
    }

    fn fill(&mut self, el: &Element, value: &str) -> Result<()> {
        let node = self.snap.nodes.get(el.handle).cloned().ok_or_else(|| anyhow!("元素已失效，请重新查找"))?;
        if !el.editable {
            bail!("{} 不是可编辑的输入框，无法填写", el.tag);
        }
        if node.can("SetValue") {
            self.with_ref(el, &|ad, r| ad.call(&["set-value", r, value], CALL))
        } else {
            self.with_ref(el, &|ad, r| ad.call(&["clear", r], CALL))?;
            if value.is_empty() {
                return Ok(());
            }
            self.with_ref(el, &|ad, r| ad.call(&["type", r, value], CALL))
        }
    }

    fn select(&mut self, el: &Element, value: &str) -> Result<String> {
        self.with_ref(el, &|ad, r| ad.call(&["select", r, value], CALL))?;
        Ok(value.to_string())
    }

    fn type_text(&mut self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let n = self.focused_input()?;
        let r = n.act_ref.unwrap_or_default();
        self.ad.call(&["type", &r, text], CALL).map(|_| ())
    }

    fn press(&mut self, key: &str) -> Result<()> {
        let c = combo(key)?;
        let mut args = vec!["press".to_string(), c];
        args.extend(self.target());
        self.ad.call(&args.iter().map(|s| s.as_str()).collect::<Vec<_>>(), CALL).map(|_| ())
    }

    fn swipe(&mut self, direction: &str) -> Result<()> {
        // 方向是手指移动的方向（与 Android 一致）：手指向上 = 内容向下滚
        let scroll = match direction.to_ascii_lowercase().as_str() {
            "up" => "down",
            "down" => "up",
            "left" => "right",
            "right" => "left",
            other => bail!("swipe 方向写 up / down / left / right（手指移动的方向），不认识 {other}"),
        };
        self.refresh()?;
        let area = self
            .snap
            .nodes
            .iter()
            .filter(|n| n.role == "scrollarea" && n.ref_id.is_some() && !n.has_state("hidden"))
            .max_by(|a, b| (a.rect.w * a.rect.h).partial_cmp(&(b.rect.w * b.rect.h)).unwrap_or(std::cmp::Ordering::Equal))
            .and_then(|n| n.ref_id.clone())
            .ok_or_else(|| anyhow!("窗口里没有可滚动的区域"))?;
        self.ad.call(&["scroll", &area, "--direction", scroll, "--amount", "5"], CALL).map(|_| ())
    }

    fn back(&mut self) -> Result<()> {
        bail!("Mac 桌面用例不支持 back：用 press 发快捷键，或点应用里的返回按钮")
    }

    fn goto(&mut self, _url: &str) -> Result<()> {
        bail!("Mac 桌面用例不支持 goto（Web 专用）")
    }

    /// 启动应用并等它出现窗口；`restart` 为真时先正常退出再启动（不强杀，避免丢失用户数据）
    fn launch(&mut self, restart: bool) -> Result<()> {
        if let Some((name, _)) = self.running() {
            self.name = name;
            if restart {
                self.quit()?;
                let deadline = Instant::now() + Duration::from_secs(10);
                while self.running().is_some() {
                    if Instant::now() >= deadline {
                        bail!("{} 没有在 10 秒内退出（可能在等你确认保存）：先在应用里处理，或把 restart 去掉直接接管它", self.name);
                    }
                    std::thread::sleep(Duration::from_millis(300));
                }
            }
        }
        let timeout = self.launch_timeout.as_millis().to_string();
        let data = self.call(&["launch", &self.app.clone(), "--activate", "--timeout", &timeout]).map_err(|e| match e.downcast_ref::<AdError>() {
            Some(a) if a.code == "APP_NOT_FOUND" => anyhow!("找不到应用「{}」：app 写应用名称（如 TextEdit）或 bundle ID（如 com.apple.TextEdit）", self.app),
            _ => e,
        })?;
        if let Some(n) = data.get("app").and_then(Value::as_str) {
            self.name = n.to_string();
        }
        self.lock_window()?;
        // 等窗口出现
        let deadline = Instant::now() + self.launch_timeout;
        loop {
            match self.page() {
                Ok(p) if !p.title.is_empty() || self.window_id.is_some() => return Ok(()),
                _ if Instant::now() >= deadline => bail!("{} {} 秒内没有出现窗口", self.name, self.launch_timeout.as_secs()),
                _ => std::thread::sleep(Duration::from_millis(250)),
            }
        }
    }

    fn stop(&mut self) -> Result<()> {
        self.quit()
    }

    fn settle(&mut self, _timeout: Duration) {
        std::thread::sleep(Duration::from_millis(300));
    }

    fn screenshot(&mut self) -> Result<Shot> {
        let (bytes, scale) = self.screenshot_png()?;
        self.scale = scale;
        // 窗口位置取自最近一次界面快照；没读过时直接给整图
        let bytes = crop_to_window(&bytes, &self.snap.window, scale).unwrap_or(bytes);
        Ok(Shot { bytes, scale })
    }

    fn attr(&mut self, el: &Element, name: &str) -> Result<Option<String>> {
        let Some(n) = self.snap.nodes.get(el.handle) else { return Ok(None) };
        Ok(match name {
            "role" => Some(n.role.clone()),
            "name" | "title" => Some(n.name.clone()),
            "value" => n.value.clone(),
            "description" | "desc" => Some(n.description.clone()),
            "id" | "identifier" => Some(n.native_id.clone()).filter(|s| !s.is_empty()),
            "states" => Some(n.states.join(",")),
            _ => None,
        })
    }

    fn failure_scene(&mut self, dir: &Path) -> Vec<Artifact> {
        let mut out = vec![];
        if self.refresh().is_err() && self.snap.nodes.is_empty() {
            self.notes.push("失败时读取界面结构也失败了".into());
        }
        let text = outline(&self.snap);
        if !self.snap.nodes.is_empty() && crate::fsx::write(dir.join("accessibility.txt"), text.as_bytes()).is_ok() {
            out.push(Artifact { label: "界面结构（无障碍树）".into(), file: "accessibility.txt".into() });
        }
        if let Ok(w) = self.call(&["list-windows", "--app", &self.name]) {
            let lines: Vec<String> = w
                .as_array()
                .into_iter()
                .flatten()
                .map(|w| format!("{}  「{}」{}", w.get("id").and_then(Value::as_str).unwrap_or_default(), w.get("title").and_then(Value::as_str).unwrap_or_default(), if w.get("is_focused").and_then(Value::as_bool) == Some(true) { "  （焦点）" } else { "" }))
                .collect();
            if !lines.is_empty() && crate::fsx::write(dir.join("windows.txt"), lines.join("\n").as_bytes()).is_ok() {
                out.push(Artifact { label: "应用的窗口".into(), file: "windows.txt".into() });
            }
        }
        out
    }

    fn environment(&mut self) -> Vec<(String, String)> {
        let sh = |cmd: &str, args: &[&str]| Command::new(cmd).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        let mut env = vec![("系统".to_string(), format!("macOS {}", sh("sw_vers", &["-productVersion"])))];
        let ver = Command::new(&self.ad.path).arg("--version").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        if !ver.is_empty() {
            env.push(("agent-desktop".into(), ver.trim_start_matches("agent-desktop ").to_string()));
        }
        if let Some((name, bundle)) = self.running() {
            let path = sh("mdfind", &[&format!("kMDItemCFBundleIdentifier == '{bundle}'")]).lines().next().unwrap_or_default().to_string();
            let version = if path.is_empty() { String::new() } else { sh("defaults", &["read", &format!("{path}/Contents/Info"), "CFBundleShortVersionString"]) };
            env.push(("应用".into(), if version.is_empty() { name } else { format!("{name} {version}") }));
        }
        if self.snap.nodes.is_empty() && self.running().is_some() {
            let _ = self.refresh();
        }
        if !self.snap.window.is_empty() {
            env.push(("窗口".into(), format!("{:.0}×{:.0}（{}）", self.snap.window.w, self.snap.window.h, self.snap.window_title)));
        }
        env
    }

    fn take_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn data() -> Value {
        json!({
            "app": "Notes", "snapshot_id": "s1", "complete": true, "window": {"id": "w-9", "title": "备忘录"},
            "tree": {"role": "window", "name": "备忘录", "bounds": {"x": 100.0, "y": 50.0, "width": 800.0, "height": 600.0}, "children": [
                {"role": "textfield", "ref_id": "@s1:e1", "available_actions": ["SetValue", "TypeText"], "states": ["focused"], "value": "买牛奶",
                 "native_id": {"kind": "ax_identifier", "value": "note-body"}, "bounds": {"x": 120.0, "y": 100.0, "width": 400.0, "height": 300.0}},
                {"role": "button", "name": "新建", "ref_id": "@s1:e2", "available_actions": ["Click"], "bounds": {"x": 110.0, "y": 60.0, "width": 40.0, "height": 20.0}},
                {"role": "checkbox", "name": "置顶", "ref_id": "@s1:e3", "available_actions": ["Click", "Toggle"], "states": ["checked"], "value": "1", "bounds": {"x": 200.0, "y": 60.0, "width": 20.0, "height": 20.0}},
                {"role": "cell", "name": "清单", "ref_id": "@s1:e4", "available_actions": ["Click"], "bounds": {"x": 100.0, "y": 200.0, "width": 100.0, "height": 30.0}, "children": [
                    {"role": "statictext", "name": "清单", "value": "清单", "bounds": {"x": 110.0, "y": 205.0, "width": 40.0, "height": 18.0}}]},
                {"role": "button", "name": "删除", "ref_id": "@s1:e5", "states": ["disabled", "offscreen"], "bounds": {"x": 0.0, "y": 0.0, "width": 0.0, "height": 0.0}}
            ]}
        })
    }

    #[test]
    fn parses_snapshot_into_window_relative_nodes() {
        let s = parse_snapshot(&data()).unwrap();
        assert_eq!((s.app.as_str(), s.window_id.as_str(), s.window_title.as_str(), s.partial), ("Notes", "w-9", "备忘录", false));
        assert_eq!(s.nodes.len(), 7);
        assert_eq!(s.nodes[1].rect, Rect { x: 20.0, y: 50.0, w: 400.0, h: 300.0 });
        assert_eq!(s.nodes[0].rect.x, 0.0);
        assert_eq!(s.nodes[1].native_id, "note-body");
        // 静态文字没有引用：用所在单元格的
        assert_eq!((s.nodes[5].ref_id.clone(), s.nodes[5].act_ref.clone()), (None, Some("@s1:e4".to_string())));
    }

    #[test]
    fn locators_match_by_role_name_text_id_and_label() {
        let s = parse_snapshot(&data()).unwrap();
        let loc = |j: &str| serde_json::from_str::<Locator>(j).unwrap();
        let hits = |l: &Locator| s.nodes.iter().enumerate().filter(|(_, n)| node_matches(n, l)).map(|(i, _)| i).collect::<Vec<_>>();
        assert_eq!(hits(&loc(r#"{"role": "button", "name": "新建"}"#)), vec![2]);
        assert_eq!(hits(&loc(r#"{"role": "textbox"}"#)), vec![1]);
        assert_eq!(hits(&loc(r#"{"id": "note-body"}"#)), vec![1]);
        assert_eq!(hits(&loc(r#"{"text": "清单"}"#)), vec![4, 5]);
        assert_eq!(hits(&loc(r#"{"role": "checkbox", "name": "置顶"}"#)), vec![3]);
        assert_eq!(hits(&loc(r#"{"role": "button", "name": "新建", "exact": true}"#)), vec![2]);
        assert!(hits(&loc(r#"{"role": "button", "name": "不存在"}"#)).is_empty());
    }

    #[test]
    fn elements_carry_state_and_visibility() {
        let mut app = App::new(AgentDesktop { path: "agent-desktop".into() }, "Notes".into(), None, Duration::from_secs(5));
        app.snap = parse_snapshot(&data()).unwrap();
        let body = app.element_of(1);
        assert!(body.editable && body.visible && body.enabled);
        assert_eq!(body.value.as_deref(), Some("买牛奶"));
        let top = app.element_of(3);
        assert_eq!(top.checked, Some(true));
        assert_eq!(app.element_of(5).text, "清单");
        let gone = app.element_of(6);
        assert!(!gone.visible && !gone.enabled);
    }

    #[test]
    fn keys_map_to_agent_desktop_combos() {
        assert_eq!(combo("Enter").unwrap(), "return");
        assert_eq!(combo("Meta+s").unwrap(), "cmd+s");
        assert_eq!(combo("Control+Shift+Z").unwrap(), "ctrl+shift+Z");
        assert_eq!(combo("ArrowDown").unwrap(), "down");
        assert_eq!(combo("cmd+,").unwrap(), "cmd+,");
        assert!(combo("").is_err());
    }

    #[test]
    fn envelopes_become_data_or_typed_errors() {
        assert_eq!(parse_envelope(br#"{"ok": true, "data": {"a": 1}}"#).unwrap(), json!({"a": 1}));
        let e = parse_envelope(br#"{"ok": false, "error": {"code": "STALE_REF", "message": "gone", "suggestion": "re-snapshot", "disposition": {"delivery": "not_delivered"}}}"#).unwrap_err();
        let a = e.downcast_ref::<AdError>().unwrap();
        assert_eq!((a.code.as_str(), a.maybe_delivered), ("STALE_REF", false));
        let e = parse_envelope(br#"{"ok": false, "error": {"code": "ACTION_FAILED", "message": "x", "disposition": {"delivery": "delivered_unverified"}}}"#).unwrap_err();
        assert!(e.downcast_ref::<AdError>().unwrap().maybe_delivered);
        assert!(parse_envelope(b"not json").is_err());
    }

    #[test]
    fn outline_hides_secrets_and_lists_states() {
        let mut s = parse_snapshot(&data()).unwrap();
        s.nodes.push(Node { role: "securetextfield".into(), name: "密码".into(), value: Some("hunter2".into()), ..Default::default() });
        let text = outline(&s);
        assert!(text.contains("button \"新建\"") && text.contains("[checked]") && text.contains("#note-body"));
        assert!(!text.contains("hunter2") && text.contains("••••••"));
    }
}
