//! Android 驱动：adb + uiautomator，不需要在设备上装任何测试框架。
//!
//! - 界面结构：`uiautomator dump`（和无障碍服务看到的一致：文字、resource-id、content-desc、可点 / 勾选状态）
//! - 操作：`input tap / swipe / keyevent / text`；中文输入需要设备装了 ADBKeyBoard 输入法
//! - 证据：`screencap` 截图，失败时留界面结构与 logcat
use super::ui::{Actionable, Artifact, Element, PageInfo, Rect, Shot, UiDriver};
use crate::config::Locator;
use anyhow::{anyhow, bail, Context, Result};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// 界面结构里的一个节点
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Node {
    pub rect: Rect,
    pub text: String,
    pub desc: String,
    pub id: String,
    pub class: String,
    pub package: String,
    pub hint: String,
    pub enabled: bool,
    pub checkable: bool,
    pub checked: bool,
    pub clickable: bool,
    pub focused: bool,
    pub visible: bool,
    pub password: bool,
    pub attrs: BTreeMap<String, String>,
}

/// 解析 `uiautomator dump` 的 XML
pub fn parse_hierarchy(xml: &str) -> Result<Vec<Node>> {
    let start = xml.find('<').ok_or_else(|| anyhow!("界面结构为空"))?;
    let end = xml.rfind('>').map(|i| i + 1).unwrap_or(xml.len());
    let doc = roxmltree::Document::parse(&xml[start..end]).context("界面结构不是合法的 XML")?;
    let mut out = vec![];
    for n in doc.descendants().filter(|n| n.has_tag_name("node")) {
        let attrs: BTreeMap<String, String> = n.attributes().map(|a| (a.name().to_string(), a.value().to_string())).collect();
        let get = |k: &str| attrs.get(k).cloned().unwrap_or_default();
        let flag = |k: &str| attrs.get(k).map(|v| v == "true").unwrap_or(false);
        let rect = parse_bounds(&get("bounds")).unwrap_or_default();
        out.push(Node {
            rect,
            text: get("text"),
            desc: get("content-desc"),
            id: get("resource-id"),
            class: get("class"),
            package: get("package"),
            hint: get("hint"),
            enabled: attrs.get("enabled").map(|v| v == "true").unwrap_or(true),
            checkable: flag("checkable"),
            checked: flag("checked"),
            clickable: flag("clickable"),
            focused: flag("focused"),
            visible: attrs.get("visible-to-user").map(|v| v == "true").unwrap_or(true) && !rect.is_empty(),
            password: flag("password"),
            attrs,
        });
    }
    Ok(out)
}

/// `[0,63][1080,210]` → Rect
pub fn parse_bounds(s: &str) -> Option<Rect> {
    let nums: Vec<f64> = s.split(|c: char| !c.is_ascii_digit() && c != '-').filter(|p| !p.is_empty()).filter_map(|p| p.parse().ok()).collect();
    match nums.as_slice() {
        [x0, y0, x1, y1] => Some(Rect { x: *x0, y: *y0, w: x1 - x0, h: y1 - y0 }),
        _ => None,
    }
}

/// 节点是否符合定位
pub fn node_matches(n: &Node, loc: &Locator) -> bool {
    let exact = loc.exact.unwrap_or(false);
    let m = |actual: &str, want: &str| super::ui::text_matches(actual, want, exact);
    if let Some(id) = &loc.id {
        let ok = n.id == *id || n.id.ends_with(&format!(":id/{id}"));
        if !ok {
            return false;
        }
    }
    if let Some(t) = &loc.text {
        if !m(&n.text, t) {
            return false;
        }
    }
    if let Some(d) = &loc.desc {
        if !m(&n.desc, d) {
            return false;
        }
    }
    if let Some(l) = &loc.label {
        // 表单标签：输入框的提示文字 / 无障碍描述
        if !(n.class.contains("EditText") && (m(&n.hint, l) || m(&n.desc, l) || m(&n.text, l))) {
            return false;
        }
    }
    if let Some(role) = &loc.role {
        if !class_has_role(&n.class, n.checkable, role) {
            return false;
        }
        if let Some(name) = &loc.name {
            if !(m(&n.text, name) || m(&n.desc, name)) {
                return false;
            }
        }
    }
    loc.has_target()
}

fn class_has_role(class: &str, checkable: bool, role: &str) -> bool {
    let c = class.rsplit('.').next().unwrap_or(class);
    match role.to_ascii_lowercase().as_str() {
        "button" => c.contains("Button") && !c.contains("Radio") && !c.contains("Toggle"),
        "textbox" => c.contains("EditText"),
        "checkbox" => c.contains("CheckBox") || (checkable && c.contains("Check")),
        "radio" => c.contains("RadioButton"),
        "switch" => c.contains("Switch") || c.contains("Toggle"),
        "img" | "image" => c.contains("Image"),
        "text" => c.contains("TextView"),
        "combobox" => c.contains("Spinner"),
        "slider" => c.contains("SeekBar"),
        other => c.to_ascii_lowercase().contains(other),
    }
}

/// adb 可执行文件：项目配置 → PATH → ANDROID_HOME / ANDROID_SDK_ROOT → 常见安装位置
pub fn find_adb(configured: Option<&str>) -> Result<PathBuf> {
    if let Some(p) = configured.filter(|s| !s.is_empty()) {
        let pb = PathBuf::from(p);
        return if pb.is_file() { Ok(pb) } else { super::cdp::which(p).ok_or_else(|| anyhow!("找不到 adb：{p}（codebaby.json 的 android.adb）")) };
    }
    if let Some(p) = super::cdp::which("adb") {
        return Ok(p);
    }
    for var in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Ok(home) = std::env::var(var) {
            let p = Path::new(&home).join("platform-tools").join(if cfg!(windows) { "adb.exe" } else { "adb" });
            if p.is_file() {
                return Ok(p);
            }
        }
    }
    // 从 Finder / 启动台打开的桌面端拿不到登录 shell 的 PATH（Homebrew 装的 adb 在 /opt/homebrew/bin），再看常见位置
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
    let local_app_data = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    if let Some(p) = first_existing(&well_known_adb(home.as_deref(), local_app_data.as_deref())) {
        return Ok(p);
    }
    bail!("没有找到 adb：安装 Android SDK Platform-Tools，或在 codebaby.json 的 android.adb 写路径")
}

/// 常见的 adb 安装位置（按优先级）：Homebrew、Android Studio 默认 SDK 目录
fn well_known_adb(home: Option<&Path>, local_app_data: Option<&Path>) -> Vec<PathBuf> {
    let exe = if cfg!(windows) { "adb.exe" } else { "adb" };
    let mut v: Vec<PathBuf> = vec![PathBuf::from("/opt/homebrew/bin").join(exe), PathBuf::from("/usr/local/bin").join(exe)];
    if let Some(h) = home {
        v.push(h.join("Library/Android/sdk/platform-tools").join(exe));
        v.push(h.join("Android/Sdk/platform-tools").join(exe));
    }
    if let Some(l) = local_app_data {
        v.push(l.join("Android/Sdk/platform-tools").join(exe));
    }
    v
}

fn first_existing(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|p| p.is_file()).cloned()
}

/// adb：可执行文件 + 可选的远程 adb server（`android.server`，相当于每条命令加 `-H 主机 -P 端口`）
#[derive(Debug, Clone)]
pub struct Adb {
    pub path: PathBuf,
    /// adb 实际连接的 server（经代理时是本地转发端口）
    pub server: Option<(String, u16)>,
    proxy: super::proxy::ProxyPolicy,
    /// 连接方式说明（远程 server / 经代理），运行环境里记录
    pub via: Option<String>,
    /// 经代理的网络设备：(本地转发序列号, 写的地址, 代理)，提示里显示写的地址
    tunneled: Option<(String, String, String)>,
}

/// `主机:端口` 形式的序列号是网络设备（远程模拟器 / 云手机 / 无线调试），可以 `adb connect`
pub fn is_network_serial(serial: &str) -> bool {
    match serial.rsplit_once(':') {
        Some((h, p)) => !h.is_empty() && !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()),
        None => false,
    }
}

fn parse_server(s: &str) -> Result<(String, u16)> {
    let s = s.trim().trim_start_matches("tcp:");
    match s.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() => Ok((h.trim_matches(['[', ']']).to_string(), p.parse().map_err(|_| anyhow!("android.server 的端口不对：{s}"))?)),
        _ if !s.is_empty() => Ok((s.to_string(), 5037)),
        _ => bail!("android.server 为空"),
    }
}

impl Adb {
    pub fn from_config(cfg: &crate::config::ui::AndroidConfig, proxy: &super::proxy::ProxyPolicy) -> Result<Self> {
        let path = find_adb(cfg.adb.as_deref())?;
        let mut server = cfg.server.as_deref().filter(|s| !s.trim().is_empty()).map(parse_server).transpose()?;
        let mut via = server.as_ref().map(|(h, p)| format!("远程 adb server {h}:{p}"));
        // adb 不支持代理：远程 server 经代理时，adb 连本地转发端口
        if let Some((h, p)) = server.clone() {
            if let Some(px) = proxy.route(&h, false) {
                let local = super::proxy::forward(&px, &h, p)?;
                via = Some(format!("远程 adb server {h}:{p}，经代理 {}", px.display()));
                server = Some(("127.0.0.1".into(), local.port()));
            }
        }
        Ok(Self { path, server, proxy: proxy.clone(), via, tunneled: None })
    }

    /// 指定的序列号 → adb 里实际用的序列号：网络设备经代理时，是到它的本地转发端口 `127.0.0.1:端口`
    /// （端口由设备地址决定，多次运行不变）
    pub fn actual_serial(&mut self, want: &str) -> Result<String> {
        if self.server.is_none() && is_network_serial(want) {
            let (h, p) = super::proxy::split_host_port(want, 5555);
            if let Some(px) = self.proxy.route(&h, false) {
                let local = super::proxy::forward(&px, &h, p)?;
                let actual = format!("127.0.0.1:{}", local.port());
                self.via = Some(format!("{want}，经代理 {}", px.display()));
                self.tunneled = Some((actual.clone(), want.to_string(), px.display()));
                return Ok(actual);
            }
        }
        Ok(want.to_string())
    }

    pub fn run(&self, args: &[&str], timeout: Duration) -> Result<Vec<u8>> {
        match &self.server {
            Some((host, port)) => {
                let port = port.to_string();
                let mut all = vec!["-H", host.as_str(), "-P", port.as_str()];
                all.extend_from_slice(args);
                run_adb(&self.path, &all, timeout).map_err(|e| {
                    if format!("{e:#}").contains("cannot connect to daemon") {
                        anyhow!("连不上远程 adb server {host}:{port}：确认那台机器用 `adb kill-server && adb -a server nodaemon` 启动了 adb，且端口从本机可达")
                    } else {
                        anyhow!("{e:#}（adb server {host}:{port}）")
                    }
                })
            }
            None => run_adb(&self.path, args, timeout),
        }
    }

    /// 已知的设备与状态（device / offline / unauthorized …）
    pub fn devices(&self) -> Result<Vec<(String, String)>> {
        let out = String::from_utf8_lossy(&self.run(&["devices"], Duration::from_secs(10))?).into_owned();
        Ok(out
            .lines()
            .skip(1)
            .filter_map(|l| {
                let mut p = l.split_whitespace();
                Some((p.next()?.to_string(), p.next()?.to_string()))
            })
            .collect())
    }

    /// 网络设备（`主机:端口`）没连上就 `adb connect`，等到可用；失败时说明原因
    pub fn ensure_connected(&self, serial: &str) -> Result<()> {
        let state = |list: &[(String, String)]| list.iter().find(|(id, _)| id == serial).map(|(_, s)| s.clone());
        let mut current = state(&self.devices()?);
        if current.as_deref() == Some("device") || !is_network_serial(serial) {
            return Ok(());
        }
        if current.as_deref() == Some("offline") {
            // 远程端重启过的旧连接会一直停在 offline：断开重连
            let _ = self.run(&["disconnect", serial], Duration::from_secs(10));
        }
        let tunnel = self.tunneled.clone().filter(|(a, _, _)| a == serial);
        let shown = tunnel.as_ref().map(|(_, w, _)| w.clone()).unwrap_or_else(|| serial.to_string());
        let out = self.run(&["connect", serial], Duration::from_secs(20)).map(|b| String::from_utf8_lossy(&b).trim().to_string())?;
        let low = out.to_ascii_lowercase();
        if !low.contains("connected to") || low.contains("failed") || low.contains("cannot") || low.contains("unable") {
            if let Some((_, w, px)) = &tunnel {
                // 别在 adb 里留下一条离线的本地转发设备
                let _ = self.run(&["disconnect", serial], Duration::from_secs(10));
                bail!("经代理 {px} 连接远程设备 {w} 失败：代理连不到这个地址，或对方没开 adb 端口\n确认代理那一侧能访问 {w}；不该走代理的地址写进 proxy.noProxy");
            }
            bail!("连接远程设备 {serial} 失败：{out}\n确认这个地址从本机可达、对方开着 adb 端口（模拟器 / 云手机控制台里打开 ADB，真机先 adb tcpip 5555 或无线调试配对）");
        }
        // connect 返回后设备要过一会儿才从 offline / authorizing 变成 device
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            current = state(&self.devices()?);
            match current.as_deref() {
                Some("device") => return Ok(()),
                Some("unauthorized") => bail!("远程设备 {shown} 未授权：在设备上允许这台电脑调试（无线调试需先 adb pair）"),
                _ if Instant::now() >= deadline => bail!("远程设备 {shown} 已连接但不可用（状态 {}）", current.as_deref().unwrap_or("不在列表里")),
                _ => std::thread::sleep(Duration::from_millis(300)),
            }
        }
    }

    /// 选定设备：ANDROID_SERIAL → 项目 android.serial → 唯一连接的设备。
    /// 指定的是 `主机:端口` 时自动 `adb connect`
    pub fn pick_serial(&mut self, configured: Option<&str>) -> Result<String> {
        if let Some(asked) = std::env::var("ANDROID_SERIAL").ok().filter(|s| !s.is_empty()).or(configured.map(String::from)) {
            let want = self.actual_serial(&asked)?;
            if is_network_serial(&want) {
                self.ensure_connected(&want)?;
            }
            let list = self.devices()?;
            return match list.iter().find(|(id, _)| *id == want) {
                Some((_, s)) if s == "device" => Ok(want),
                Some((_, s)) => bail!("设备 {want} 状态为 {s}{}", if s == "unauthorized" { "：在设备上允许 USB 调试" } else { "" }),
                None => bail!("设备 {want} 没有连接（adb devices 里没有）"),
            };
        }
        let list = self.devices()?;
        let ready: Vec<&String> = list.iter().filter(|(_, s)| s == "device").map(|(id, _)| id).collect();
        match ready.as_slice() {
            [one] => Ok(one.to_string()),
            [] if list.iter().any(|(_, s)| s == "unauthorized") => bail!("设备未授权：在手机上允许 USB 调试"),
            [] => bail!("没有连接 Android 设备：连接手机并打开 USB 调试，或启动模拟器（adb devices 查看）；远程模拟器 / 云手机在 android.serial 写 主机:端口，会自动连接"),
            many => bail!("连接了 {} 台设备（{}）：用 ANDROID_SERIAL 或 codebaby.json 的 android.serial 指定一台", many.len(), many.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("、")),
        }
    }
}

/// 运行 adb，超时即结束进程
pub fn run_adb(adb: &Path, args: &[&str], timeout: Duration) -> Result<Vec<u8>> {
    let mut child = Command::new(adb).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().with_context(|| format!("运行 adb 失败：{}", adb.display()))?;
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
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("adb {} 超过 {} 秒没有结束", args.join(" "), timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let out = out.join().unwrap_or_default();
    let err = String::from_utf8_lossy(&err.join().unwrap_or_default()).trim().to_string();
    if !status.success() {
        bail!("adb {} 失败：{}", args.join(" "), if err.is_empty() { String::from_utf8_lossy(&out).trim().to_string() } else { err });
    }
    Ok(out)
}

/// 一台设备上的一个应用
pub struct Device {
    adb: Adb,
    pub serial: String,
    pub app: String,
    pub activity: Option<String>,
    pub size: (u32, u32),
    nodes: Vec<Node>,
    last_xml: String,
    /// 用例开始时的设备时间（logcat 从这里开始取）
    since: String,
    launch_timeout: Duration,
    notes: Vec<String>,
}

const ADB: Duration = Duration::from_secs(30);

impl Device {
    /// 当前界面的节点（快照用）
    pub fn nodes(&mut self) -> Result<Vec<Node>> {
        self.dump()?;
        Ok(self.nodes.clone())
    }

    pub fn connect(adb: Adb, serial: String, app: String, activity: Option<String>, launch_timeout: Duration) -> Result<Self> {
        let mut d = Device { adb, serial, app, activity, size: (0, 0), nodes: vec![], last_xml: String::new(), since: String::new(), launch_timeout, notes: vec![] };
        let size = d.shell("wm size")?;
        // Override size（改过分辨率）优先
        let line = size.lines().rev().find(|l| l.contains("size:")).unwrap_or_default().to_string();
        if let Some((w, h)) = line.rsplit(' ').next().and_then(|s| s.trim().split_once('x')) {
            d.size = (w.parse().unwrap_or(0), h.parse().unwrap_or(0));
        }
        d.since = d.shell("date '+%m-%d %H:%M:%S.000'").unwrap_or_default().trim().to_string();
        Ok(d)
    }

    fn adb(&self, args: &[&str], timeout: Duration) -> Result<Vec<u8>> {
        let mut all = vec!["-s", self.serial.as_str()];
        all.extend_from_slice(args);
        self.adb.run(&all, timeout)
    }

    pub fn shell(&self, cmd: &str) -> Result<String> {
        Ok(String::from_utf8_lossy(&self.adb(&["shell", cmd], ADB)?).into_owned())
    }

    pub fn install(&self, apk: &Path) -> Result<()> {
        let p = apk.to_string_lossy();
        self.adb(&["install", "-r", "-t", &p], Duration::from_secs(300)).map(|_| ())
    }

    /// 抓取当前界面结构（界面在动画中时 uiautomator 会拒绝，稍后重试）
    fn dump(&mut self) -> Result<()> {
        let mut last = String::new();
        for _ in 0..3 {
            let out = self.adb(&["exec-out", "uiautomator dump /dev/tty"], ADB);
            match out {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes).into_owned();
                    if text.contains("<hierarchy") {
                        self.nodes = parse_hierarchy(&text)?;
                        self.last_xml = text[text.find('<').unwrap_or(0)..text.rfind('>').map(|i| i + 1).unwrap_or(text.len())].to_string();
                        return Ok(());
                    }
                    last = text.trim().to_string();
                }
                Err(e) => last = format!("{e:#}"),
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        bail!("读取界面结构失败：{last}")
    }

    /// 当前前台的包名与 Activity
    fn focus(&self) -> Result<(String, String)> {
        let out = self.shell("dumpsys window | grep -E 'mCurrentFocus|mFocusedApp'")?;
        for line in out.lines() {
            // mCurrentFocus=Window{3c2 u0 com.x.shop/com.x.shop.MainActivity}
            if let Some(comp) = line.split_whitespace().find(|w| w.contains('/')) {
                let comp = comp.trim_end_matches('}');
                if let Some((pkg, act)) = comp.split_once('/') {
                    let act = if let Some(rest) = act.strip_prefix('.') { format!("{pkg}.{rest}") } else { act.to_string() };
                    return Ok((pkg.to_string(), act));
                }
            }
        }
        Ok((String::new(), String::new()))
    }

    fn tap_xy(&self, x: f64, y: f64) -> Result<()> {
        self.shell(&format!("input tap {} {}", x.round() as i64, y.round() as i64)).map(|_| ())
    }

    fn keycodes(&self, codes: &[&str]) -> Result<()> {
        self.shell(&format!("input keyevent {}", codes.join(" "))).map(|_| ())
    }

    fn input_text(&self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        if text.is_ascii() {
            // input text：空格写 %s，其余 shell 特殊字符逐个转义
            let mut esc = String::new();
            for c in text.chars() {
                match c {
                    ' ' => esc.push_str("%s"),
                    c if c.is_ascii_alphanumeric() || matches!(c, '.' | ',' | '-' | '_' | '@' | '/' | ':' | '+' | '=') => esc.push(c),
                    c => {
                        esc.push('\\');
                        esc.push(c);
                    }
                }
            }
            return self.shell(&format!("input text {esc}")).map(|_| ());
        }
        // 中文等：借助 ADBKeyBoard 输入法（https://github.com/senzhk/ADBKeyBoard）
        let ime = "com.android.adbkeyboard/.AdbIME";
        let has = self.shell("ime list -s").unwrap_or_default();
        if !has.contains(ime) {
            bail!("输入非英文字符需要设备安装 ADBKeyBoard 输入法（github.com/senzhk/ADBKeyBoard），安装后无需手动切换");
        }
        let prev = self.shell("settings get secure default_input_method").unwrap_or_default().trim().to_string();
        self.shell(&format!("ime enable {ime} >/dev/null; ime set {ime} >/dev/null"))?;
        std::thread::sleep(Duration::from_millis(300));
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
        let r = self.shell(&format!("am broadcast -a ADB_INPUT_B64 --es msg {b64}"));
        if !prev.is_empty() && prev != ime && prev != "null" {
            let _ = self.shell(&format!("ime set {prev} >/dev/null"));
        }
        r.map(|_| ())
    }

    fn element_of(&self, i: usize) -> Element {
        let n = &self.nodes[i];
        Element {
            handle: i,
            rect: n.rect,
            visible: n.visible && self.on_screen(&n.rect),
            enabled: n.enabled,
            checked: n.checkable.then_some(n.checked),
            editable: n.class.contains("EditText"),
            readonly: false,
            text: if n.text.is_empty() { n.desc.clone() } else { n.text.clone() },
            value: (n.class.contains("EditText") && !n.password).then(|| n.text.clone()),
            secret: n.password,
            tag: format!("{}{}", n.class.rsplit('.').next().unwrap_or(&n.class), if n.id.is_empty() { String::new() } else { format!(" #{}", n.id.rsplit('/').next().unwrap_or(&n.id)) }),
            ..Default::default()
        }
    }

    fn on_screen(&self, r: &Rect) -> bool {
        if self.size.0 == 0 {
            return true;
        }
        let (cx, cy) = r.center();
        cx >= 0.0 && cy >= 0.0 && cx < self.size.0 as f64 && cy < self.size.1 as f64
    }
}

/// 按键名 → Android keycode
pub(crate) fn keycode(key: &str) -> Result<String> {
    let k = key.trim();
    if k.starts_with("KEYCODE_") {
        return Ok(k.to_string());
    }
    let code = match k.to_ascii_lowercase().as_str() {
        "enter" => "KEYCODE_ENTER",
        "tab" => "KEYCODE_TAB",
        "escape" | "esc" => "KEYCODE_ESCAPE",
        "backspace" => "KEYCODE_DEL",
        "delete" => "KEYCODE_FORWARD_DEL",
        "space" => "KEYCODE_SPACE",
        "back" => "KEYCODE_BACK",
        "home" => "KEYCODE_HOME",
        "menu" => "KEYCODE_MENU",
        "search" => "KEYCODE_SEARCH",
        "appswitch" | "recents" => "KEYCODE_APP_SWITCH",
        "arrowup" | "up" => "KEYCODE_DPAD_UP",
        "arrowdown" | "down" => "KEYCODE_DPAD_DOWN",
        "arrowleft" | "left" => "KEYCODE_DPAD_LEFT",
        "arrowright" | "right" => "KEYCODE_DPAD_RIGHT",
        "volumeup" => "KEYCODE_VOLUME_UP",
        "volumedown" => "KEYCODE_VOLUME_DOWN",
        "power" => "KEYCODE_POWER",
        _ => bail!("不认识的按键 {k}（Enter / Back / Home / Tab / Backspace / KEYCODE_* …）"),
    };
    Ok(code.into())
}

impl UiDriver for Device {
    fn kind(&self) -> crate::config::UiKind {
        crate::config::UiKind::Android
    }

    fn find(&mut self, loc: &Locator) -> Result<Vec<Element>> {
        if loc.css.is_some() || loc.test_id.is_some() || loc.placeholder.is_some() {
            bail!("Android 用 id / text / desc / label / role 定位（css / testId / placeholder 只用于 Web）");
        }
        self.dump()?;
        Ok((0..self.nodes.len()).filter(|i| node_matches(&self.nodes[*i], loc)).map(|i| self.element_of(i)).collect())
    }

    fn page(&mut self) -> Result<PageInfo> {
        let (pkg, act) = self.focus()?;
        Ok(PageInfo { location: pkg, title: act })
    }

    fn actionable(&mut self, el: &Element) -> Result<Actionable> {
        if !self.on_screen(&el.rect) {
            return Ok(Actionable::NotYet("不在屏幕内（先 swipe 滑到它）".into()));
        }
        Ok(Actionable::Ok(el.clone()))
    }

    fn click(&mut self, el: &Element) -> Result<()> {
        let (x, y) = el.rect.center();
        self.tap_xy(x, y)
    }

    fn long_press(&mut self, el: &Element) -> Result<()> {
        let (x, y) = el.rect.center();
        let (x, y) = (x.round() as i64, y.round() as i64);
        self.shell(&format!("input swipe {x} {y} {x} {y} 800")).map(|_| ())
    }

    fn hover(&mut self, _el: &Element) -> Result<()> {
        bail!("Android 用例不支持 hover（Web 专用）")
    }

    fn fill(&mut self, el: &Element, value: &str) -> Result<()> {
        if !el.editable {
            bail!("{} 不是输入框，无法填写", el.tag);
        }
        let (x, y) = el.rect.center();
        self.tap_xy(x, y)?;
        std::thread::sleep(Duration::from_millis(200));
        // 清空：移到末尾后按删除键，次数取当前文字长度（提示文字算在内也无妨）
        let n = el.value.as_deref().unwrap_or_default().chars().count().max(1) + 2;
        let mut codes = vec!["KEYCODE_MOVE_END"];
        codes.extend(std::iter::repeat_n("KEYCODE_DEL", n));
        self.keycodes(&codes)?;
        self.input_text(value)
    }

    fn select(&mut self, _el: &Element, _value: &str) -> Result<String> {
        bail!("Android 用例不支持 select：点开下拉框后 tap 选项")
    }

    fn type_text(&mut self, text: &str) -> Result<()> {
        self.input_text(text)
    }

    fn press(&mut self, key: &str) -> Result<()> {
        let code = keycode(key)?;
        self.keycodes(&[&code])
    }

    fn swipe(&mut self, direction: &str) -> Result<()> {
        let (w, h) = (self.size.0.max(1) as f64, self.size.1.max(1) as f64);
        let (x0, y0, x1, y1) = match direction.to_ascii_lowercase().as_str() {
            "up" => (w / 2.0, h * 0.75, w / 2.0, h * 0.25),
            "down" => (w / 2.0, h * 0.25, w / 2.0, h * 0.75),
            "left" => (w * 0.8, h / 2.0, w * 0.2, h / 2.0),
            "right" => (w * 0.2, h / 2.0, w * 0.8, h / 2.0),
            other => bail!("swipe 方向写 up / down / left / right（手指移动的方向），不认识 {other}"),
        };
        self.shell(&format!("input swipe {} {} {} {} 300", x0 as i64, y0 as i64, x1 as i64, y1 as i64)).map(|_| ())
    }

    fn back(&mut self) -> Result<()> {
        self.keycodes(&["KEYCODE_BACK"])
    }

    fn goto(&mut self, _url: &str) -> Result<()> {
        bail!("Android 用例不支持 goto（Web 专用）")
    }

    fn launch(&mut self, clear_data: bool) -> Result<()> {
        let app = self.app.clone();
        if clear_data {
            let out = self.shell(&format!("pm clear {app}"))?;
            if !out.contains("Success") {
                bail!("清除 {app} 的数据失败：{}", out.trim());
            }
        } else {
            self.shell(&format!("am force-stop {app}"))?;
        }
        let out = match &self.activity {
            Some(act) => {
                let comp = if act.contains('/') { act.clone() } else { format!("{app}/{act}") };
                self.shell(&format!("am start -W -n {comp}"))?
            }
            None => self.shell(&format!("monkey -p {app} -c android.intent.category.LAUNCHER 1"))?,
        };
        if out.contains("Error") || out.contains("No activities found") || out.contains("monkey aborted") {
            let installed = self.shell(&format!("pm path {app}")).unwrap_or_default();
            if installed.trim().is_empty() {
                bail!("设备上没有安装 {app}（用例的 apk 字段可以先安装）");
            }
            bail!("启动 {app} 失败：{}", out.lines().find(|l| l.contains("Error") || l.contains("aborted") || l.contains("No activities")).unwrap_or(out.trim()));
        }
        // 等应用来到前台
        let deadline = Instant::now() + self.launch_timeout;
        loop {
            if let Ok((pkg, _)) = self.focus() {
                if pkg == app {
                    return Ok(());
                }
            }
            if Instant::now() >= deadline {
                bail!("{app} {} 秒内没有来到前台", self.launch_timeout.as_secs());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    fn attach(&mut self) -> Result<()> {
        let (pkg, _) = self.focus()?;
        if pkg == self.app {
            return Ok(());
        }
        if pkg.is_empty() {
            bail!("设备上没有前台应用（锁屏了？）：先解锁并打开 {}，再运行（attach 不会启动应用）", self.app);
        }
        bail!("设备前台是 {pkg}，不是 {}：先在设备上打开要检查的页面，再运行（attach 不会启动应用）", self.app)
    }

    fn stop(&mut self) -> Result<()> {
        let app = self.app.clone();
        self.shell(&format!("am force-stop {app}")).map(|_| ())
    }

    fn settle(&mut self, _timeout: Duration) {
        std::thread::sleep(Duration::from_millis(400));
    }

    fn screenshot(&mut self) -> Result<Shot> {
        let bytes = self.adb(&["exec-out", "screencap -p"], ADB)?;
        if !bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
            bail!("截图失败（设备锁屏或应用禁止截图时会这样）");
        }
        Ok(Shot { bytes, scale: 1.0 })
    }

    fn attr(&mut self, el: &Element, name: &str) -> Result<Option<String>> {
        Ok(self.nodes.get(el.handle).and_then(|n| n.attrs.get(name).cloned()))
    }

    fn failure_scene(&mut self, dir: &Path) -> Vec<Artifact> {
        let mut out = vec![];
        if self.dump().is_err() && self.last_xml.is_empty() {
            self.notes.push("失败时读取界面结构也失败了".into());
        }
        let mut save = |label: &str, file: &str, content: &str| {
            if !content.is_empty() && crate::fsx::write(dir.join(file), content.as_bytes()).is_ok() {
                out.push(Artifact { label: label.into(), file: file.into() });
            }
        };
        save("界面结构（uiautomator）", "window.xml", &self.last_xml);
        let since = self.since.clone();
        let log = self.adb(&["logcat", "-d", "-v", "time", "-T", &since], ADB).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        let tail: Vec<&str> = log.lines().collect();
        let tail = tail[tail.len().saturating_sub(5000)..].join("\n");
        save("logcat（本用例期间）", "logcat.txt", &tail);
        let crash = self.adb(&["logcat", "-d", "-b", "crash", "-T", &since], ADB).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        save("崩溃日志", "crash.txt", crash.trim());
        out
    }

    fn environment(&mut self) -> Vec<(String, String)> {
        let prop = |k: &str| self.shell(&format!("getprop {k}")).map(|s| s.trim().to_string()).unwrap_or_default();
        let model = format!("{} {}", prop("ro.product.manufacturer"), prop("ro.product.model"));
        let android = format!("Android {}（API {}）", prop("ro.build.version.release"), prop("ro.build.version.sdk"));
        let app = self.app.clone();
        let version = self
            .shell(&format!("dumpsys package {app} | grep -m1 versionName"))
            .map(|s| s.trim().trim_start_matches("versionName=").to_string())
            .unwrap_or_default();
        let mut env = vec![
            ("设备".into(), model.trim().to_string()),
            ("系统".into(), android),
            ("序列号".into(), self.serial.clone()),
        ];
        if let Some(v) = &self.adb.via {
            env.push(("连接".into(), v.clone()));
        } else if is_network_serial(&self.serial) {
            env.push(("连接".into(), "网络（adb connect）".into()));
        }
        env.push(("屏幕".into(), format!("{}×{}", self.size.0, self.size.1)));
        if !version.is_empty() {
            env.push(("应用版本".into(), format!("{app} {version}")));
        }
        env
    }

    fn take_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn adb_is_found_in_well_known_locations_when_path_has_none() {
        let home = tempfile::tempdir().unwrap();
        let sdk = home.path().join("Library/Android/sdk/platform-tools");
        std::fs::create_dir_all(&sdk).unwrap();
        let exe = if cfg!(windows) { "adb.exe" } else { "adb" };
        let cands = super::well_known_adb(Some(home.path()), None);
        assert!(cands.contains(&sdk.join(exe)) && cands.iter().any(|p| p.starts_with("/opt/homebrew/bin")), "{cands:?}");
        assert_eq!(super::first_existing(&cands).filter(|p| p.starts_with(home.path())), None, "没有 adb 文件时不能当作找到");
        std::fs::write(sdk.join(exe), "").unwrap();
        assert_eq!(super::first_existing(&[sdk.join(exe)]), Some(sdk.join(exe)));
    }

    use super::*;

    #[test]
    fn network_serials_and_adb_server() {
        assert!(is_network_serial("192.168.1.20:5555"));
        assert!(is_network_serial("cloud.example.com:40001"));
        assert!(!is_network_serial("emulator-5554"));
        assert!(!is_network_serial("R58M123ABC"));
        assert!(!is_network_serial("adb-R58M._adb-tls-connect._tcp"));
        assert_eq!(parse_server("10.0.0.8").unwrap(), ("10.0.0.8".into(), 5037));
        assert_eq!(parse_server("tcp:10.0.0.8:5038").unwrap(), ("10.0.0.8".into(), 5038));
        assert!(parse_server("host:abc").is_err());
    }

    const XML: &str = r#"<?xml version='1.0' encoding='UTF-8' standalone='yes' ?><hierarchy rotation="0"><node index="0" text="" resource-id="" class="android.widget.FrameLayout" package="com.x.shop" content-desc="" checkable="false" checked="false" clickable="false" enabled="true" focusable="false" focused="false" scrollable="false" long-clickable="false" password="false" selected="false" bounds="[0,0][1080,2400]"><node index="0" text="" resource-id="com.x.shop:id/phone" class="android.widget.EditText" package="com.x.shop" content-desc="" hint="手机号" checkable="false" checked="false" clickable="true" enabled="true" focusable="true" focused="false" scrollable="false" long-clickable="true" password="false" selected="false" bounds="[60,400][1020,520]" /><node index="1" text="登录" resource-id="com.x.shop:id/login" class="android.widget.Button" package="com.x.shop" content-desc="" checkable="false" checked="false" clickable="true" enabled="false" focusable="true" focused="false" scrollable="false" long-clickable="false" password="false" selected="false" bounds="[60,600][1020,720]" /><node index="2" text="记住我" resource-id="" class="android.widget.CheckBox" package="com.x.shop" content-desc="" checkable="true" checked="true" clickable="true" enabled="true" focusable="true" focused="false" scrollable="false" long-clickable="false" password="false" selected="false" bounds="[60,760][400,840]" /></node></hierarchy>UI hierchary dumped to: /dev/tty"#;

    #[test]
    fn parses_dump_and_matches_locators() {
        let nodes = parse_hierarchy(XML).unwrap();
        assert_eq!(nodes.len(), 4);
        assert_eq!(nodes[1].rect, Rect { x: 60.0, y: 400.0, w: 960.0, h: 120.0 });
        let loc = |j: &str| serde_json::from_str::<Locator>(j).unwrap();
        let hits = |l: &Locator| nodes.iter().enumerate().filter(|(_, n)| node_matches(n, l)).map(|(i, _)| i).collect::<Vec<_>>();
        assert_eq!(hits(&loc(r#"{"id": "phone"}"#)), vec![1]);
        assert_eq!(hits(&loc(r#"{"id": "com.x.shop:id/phone"}"#)), vec![1]);
        assert_eq!(hits(&loc(r#"{"label": "手机号"}"#)), vec![1]);
        assert_eq!(hits(&loc(r#"{"text": "登录"}"#)), vec![2]);
        assert_eq!(hits(&loc(r#"{"role": "button", "name": "登录"}"#)), vec![2]);
        assert_eq!(hits(&loc(r#"{"role": "checkbox"}"#)), vec![3]);
        assert!(!nodes[2].enabled && nodes[3].checked);
        assert_eq!(parse_bounds("[0,63][1080,210]"), Some(Rect { x: 0.0, y: 63.0, w: 1080.0, h: 147.0 }));
        assert_eq!(keycode("Back").unwrap(), "KEYCODE_BACK");
        assert!(keycode("Hyper").is_err());
    }
}
