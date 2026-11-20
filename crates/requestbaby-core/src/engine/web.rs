//! Web 驱动：一个用例 = 一个独立的浏览器上下文里的一个标签页（Chrome DevTools 协议）。
//!
//! - 按角色 + 名称 / 表单标签定位走浏览器自己的无障碍树（和读屏软件看到的一致）；
//!   文字 / 占位文字 / testId / CSS 走页面内脚本
//! - 点击用真实的鼠标事件（先滚动到可见、等位置稳定、确认没有被遮挡），输入用真实的输入事件
//! - 一路记下控制台、网络请求与主文档的 HTTP 状态，失败时连同页面结构一起留作现场
use super::cdp::{Browser, Event};
use super::ui::{Actionable, Artifact, Element, PageInfo, Rect, Shot, UiDriver};
use crate::config::Locator;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::path::Path;
use std::time::{Duration, Instant};

/// 单条命令的等待上限（页面卡死时不至于一直挂着）
const CALL: Duration = Duration::from_secs(30);
const GROUP: &str = "rb";

pub struct WebPage<'b> {
    b: &'b mut Browser,
    context: String,
    session: String,
    frame: String,
    pub viewport: (u32, u32),
    /// 打开页面的超时（项目 timeoutMs）
    nav_timeout: Duration,
    /// 最近一次查找结果（数组对象），元素 handle 是其中的下标
    found: Option<String>,
    /// 主文档正在加载
    loading: bool,
    /// 动作触发了跳转请求、还没开始加载
    nav_pending: bool,
    /// 主文档：HTTP 状态 / 响应头 / 地址
    pub doc_status: u16,
    pub doc_headers: Vec<(String, String)>,
    console: Vec<String>,
    network: Vec<String>,
    requests: std::collections::HashMap<String, String>,
    notes: Vec<String>,
    started: Instant,
    /// 为沿用登录态注入网页存储的脚本：第一次打开页面后撤掉，之后页面自己怎么改存储都不再干预
    storage_script: Option<String>,
}

impl<'b> WebPage<'b> {
    pub fn open(b: &'b mut Browser, viewport: (u32, u32), nav_timeout: Duration) -> Result<Self> {
        let ctx = b.cdp.call(None, "Target.createBrowserContext", json!({ "disposeOnDetach": true }), CALL)?;
        let context = str_of(&ctx, "browserContextId")?;
        let t = b.cdp.call(None, "Target.createTarget", json!({ "url": "about:blank", "browserContextId": context }), CALL)?;
        let target = str_of(&t, "targetId")?;
        let a = b.cdp.call(None, "Target.attachToTarget", json!({ "targetId": target, "flatten": true }), CALL)?;
        let session = str_of(&a, "sessionId")?;
        let mut page = WebPage {
            b,
            context,
            session,
            frame: String::new(),
            viewport,
            nav_timeout,
            found: None,
            loading: false,
            nav_pending: false,
            doc_status: 0,
            doc_headers: vec![],
            console: vec![],
            network: vec![],
            requests: Default::default(),
            notes: vec![],
            started: Instant::now(),
            storage_script: None,
        };
        for m in ["Page.enable", "Runtime.enable", "Network.enable", "Log.enable", "DOM.enable", "Accessibility.enable"] {
            page.call(m, json!({}))?;
        }
        let tree = page.call("Page.getFrameTree", json!({}))?;
        page.frame = tree["frameTree"]["frame"]["id"].as_str().unwrap_or_default().to_string();
        page.call(
            "Emulation.setDeviceMetricsOverride",
            json!({ "width": viewport.0, "height": viewport.1, "deviceScaleFactor": 1, "mobile": false }),
        )?;
        Ok(page)
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let s = self.session.clone();
        self.b.cdp.call(Some(&s), method, params, CALL)
    }

    /// 处理已到达的事件：主文档状态、加载进度、控制台、网络、对话框
    fn process(&mut self) {
        for ev in self.b.cdp.take_events() {
            if ev.session.as_deref() != Some(self.session.as_str()) {
                continue;
            }
            self.on_event(ev);
        }
    }

    fn on_event(&mut self, ev: Event) {
        let p = &ev.params;
        let t = self.started.elapsed().as_millis();
        let s = |v: &Value| v.as_str().unwrap_or_default().to_string();
        match ev.method.as_str() {
            "Page.frameStartedLoading" if s(&p["frameId"]) == self.frame => {
                self.loading = true;
                self.nav_pending = false;
            }
            "Page.frameRequestedNavigation" if s(&p["frameId"]) == self.frame => self.nav_pending = true,
            "Page.frameStoppedLoading" if s(&p["frameId"]) == self.frame => self.loading = false,
            "Page.loadEventFired" => self.loading = false,
            "Page.frameNavigated" if p["frame"].get("parentId").is_none() => {
                self.frame = s(&p["frame"]["id"]);
            }
            "Page.javascriptDialogOpening" => {
                self.notes.push(format!("自动确认了{}「{}」", dialog_label(&s(&p["type"])), s(&p["message"])));
            }
            "Page.windowOpen" => self.notes.push(format!("页面打开了新窗口 {}（仍在原标签页继续）", s(&p["url"]))),
            "Network.requestWillBeSent" => {
                let id = s(&p["requestId"]);
                let line = format!("{} {}", s(&p["request"]["method"]), s(&p["request"]["url"]));
                self.requests.insert(id, line.clone());
                self.push_net(format!("{t:>6}ms → {line}"));
            }
            "Network.responseReceived" => {
                let r = &p["response"];
                let status = r["status"].as_u64().unwrap_or(0) as u16;
                if s(&p["type"]) == "Document" && s(&p["frameId"]) == self.frame {
                    self.doc_status = status;
                    self.doc_headers = r["headers"]
                        .as_object()
                        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string())).collect())
                        .unwrap_or_default();
                }
                self.push_net(format!("{t:>6}ms ← {status} {}", s(&r["url"])));
            }
            "Network.loadingFailed" => {
                let line = self.requests.get(&s(&p["requestId"])).cloned().unwrap_or_default();
                if p["canceled"].as_bool() != Some(true) {
                    self.push_net(format!("{t:>6}ms ✘ {line} {}", s(&p["errorText"])));
                }
            }
            "Runtime.consoleAPICalled" => {
                let args: Vec<String> = p["args"]
                    .as_array()
                    .map(|a| a.iter().map(|x| x.get("value").map(|v| v.as_str().map(String::from).unwrap_or_else(|| v.to_string())).unwrap_or_else(|| s(&x["description"]))).collect())
                    .unwrap_or_default();
                self.push_console(format!("{t:>6}ms [{}] {}", s(&p["type"]), args.join(" ")));
            }
            "Runtime.exceptionThrown" => {
                let d = &p["exceptionDetails"];
                let text = d["exception"]["description"].as_str().map(String::from).unwrap_or_else(|| s(&d["text"]));
                self.push_console(format!("{t:>6}ms [未捕获异常] {text}"));
            }
            "Log.entryAdded" => {
                let e = &p["entry"];
                self.push_console(format!("{t:>6}ms [{}] {} {}", s(&e["level"]), s(&e["text"]), s(&e["url"])));
            }
            "Inspector.targetCrashed" => self.notes.push("页面崩溃了".into()),
            _ => {}
        }
    }

    fn push_net(&mut self, line: String) {
        if self.network.len() < 2000 {
            self.network.push(line);
        }
    }

    fn push_console(&mut self, line: String) {
        if self.console.len() < 2000 {
            self.console.push(line);
        }
    }

    /// 页面里执行表达式，返回值（JSON）
    fn eval(&mut self, expr: &str) -> Result<Value> {
        let r = self.call("Runtime.evaluate", json!({ "expression": expr, "returnByValue": true, "awaitPromise": true }))?;
        if let Some(e) = r.get("exceptionDetails") {
            bail!("页面脚本出错：{}", exception_text(e));
        }
        Ok(r["result"].get("value").cloned().unwrap_or(Value::Null))
    }

    /// 在最近一次找到的元素数组上调用函数（this = 数组，第一个参数 = 下标）
    fn on_found(&mut self, func: &str, args: Vec<Value>, await_promise: bool) -> Result<Value> {
        let arr = self.found.clone().ok_or_else(|| anyhow!("元素已失效，需要重新查找"))?;
        let args: Vec<Value> = args.into_iter().map(|v| json!({ "value": v })).collect();
        let r = self.call(
            "Runtime.callFunctionOn",
            json!({ "objectId": arr, "functionDeclaration": func, "arguments": args, "returnByValue": true, "awaitPromise": await_promise }),
        )?;
        if let Some(e) = r.get("exceptionDetails") {
            bail!("页面脚本出错：{}", exception_text(e));
        }
        Ok(r["result"].get("value").cloned().unwrap_or(Value::Null))
    }

    /// 等主文档加载完成
    fn wait_loaded(&mut self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            self.process();
            if !self.loading {
                return Ok(());
            }
            if Instant::now() >= deadline {
                bail!("页面 {} 秒内没有加载完成", timeout.as_secs());
            }
            self.b.cdp.pump(Duration::from_millis(50))?;
        }
    }

    /// 动作开始前：清掉上一次的跳转状态
    fn begin_action(&mut self) {
        self.process();
        self.nav_pending = false;
    }

    fn mouse(&mut self, kind: &str, x: f64, y: f64, button: &str, clicks: u32) -> Result<()> {
        let buttons = if kind == "mousePressed" { 1 } else { 0 };
        self.call(
            "Input.dispatchMouseEvent",
            json!({ "type": kind, "x": x, "y": y, "button": button, "buttons": buttons, "clickCount": clicks }),
        )?;
        Ok(())
    }

    fn point_of(&self, el: &Element) -> (f64, f64) {
        let (x, y) = el.rect.center();
        (x.clamp(1.0, self.viewport.0 as f64 - 1.0), y.clamp(1.0, self.viewport.1 as f64 - 1.0))
    }

    /// 浏览器上下文里的 Cookie（运行结束后带回会话：后面的 HTTP 用例能用浏览器里登录得到的 Cookie）
    pub fn cookies(&mut self) -> Vec<crate::cookies::Cookie> {
        let ctx = self.context.clone();
        let Ok(r) = self.b.cdp.call(None, "Storage.getCookies", json!({ "browserContextId": ctx }), CALL) else { return vec![] };
        r["cookies"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|c| {
                        let domain = c["domain"].as_str().unwrap_or_default();
                        let expires = c["expires"].as_f64().filter(|e| *e > 0.0).map(|e| (e * 1000.0) as u64);
                        crate::cookies::Cookie {
                            name: c["name"].as_str().unwrap_or_default().into(),
                            value: c["value"].as_str().unwrap_or_default().into(),
                            domain: domain.trim_start_matches('.').to_lowercase(),
                            host_only: !domain.starts_with('.'),
                            path: c["path"].as_str().unwrap_or("/").into(),
                            secure: c["secure"].as_bool().unwrap_or(false),
                            expires_at_ms: expires,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 把会话里已有的 Cookie（如前面 HTTP 登录用例拿到的）放进浏览器
    pub fn set_cookies(&mut self, cookies: &[crate::cookies::Cookie]) -> Result<()> {
        if cookies.is_empty() {
            return Ok(());
        }
        let list: Vec<Value> = cookies
            .iter()
            .map(|c| {
                let mut v = json!({ "name": c.name, "value": c.value, "path": c.path, "secure": c.secure });
                if c.host_only {
                    v["url"] = json!(format!("{}://{}{}", if c.secure { "https" } else { "http" }, c.domain, c.path));
                } else {
                    v["domain"] = json!(format!(".{}", c.domain));
                }
                if let Some(e) = c.expires_at_ms {
                    v["expires"] = json!(e as f64 / 1000.0);
                }
                v
            })
            .collect();
        let ctx = self.context.clone();
        self.b.cdp.call(None, "Storage.setCookies", json!({ "cookies": list, "browserContextId": ctx }), CALL)?;
        Ok(())
    }

    /// 把会话里已有的网页存储（localStorage / sessionStorage）放进页面：在页面自己的脚本启动前写入，
    /// 只写与当前站点（origin）匹配的那份，且只在第一次打开页面时生效（前端应用因此认为「已登录」）
    pub fn inject_storage(&mut self, storage: &crate::cookies::WebStorage) -> Result<()> {
        if storage.is_empty() {
            return Ok(());
        }
        let map: serde_json::Map<String, Value> = storage
            .iter()
            .map(|(origin, s)| (origin.clone(), json!({ "local": s.local, "session": s.session })))
            .collect();
        let source = format!(
            "(()=>{{try{{const all={};const e=all[location.origin];if(!e)return;\
             for(const [k,v] of Object.entries(e.local||{{}}))localStorage.setItem(k,v);\
             for(const [k,v] of Object.entries(e.session||{{}}))sessionStorage.setItem(k,v);}}catch(_){{}}}})();",
            Value::Object(map)
        );
        let r = self.call("Page.addScriptToEvaluateOnNewDocument", json!({ "source": source }))?;
        self.storage_script = r["identifier"].as_str().map(String::from);
        Ok(())
    }

    /// 页面当前站点的网页存储（用例结束时取走，给后面的用例沿用）；about:blank / 错误页没有，返回 None
    pub fn read_storage(&mut self) -> Option<(String, crate::cookies::OriginStorage)> {
        let expr = "(()=>{try{if(!['http:','https:','file:'].includes(location.protocol))return null;\
            return JSON.stringify({o:location.origin,l:Object.fromEntries(Object.entries(localStorage)),s:Object.fromEntries(Object.entries(sessionStorage))});}catch(_){return null;}})()";
        let text = self.eval(expr).ok()?;
        let v: Value = serde_json::from_str(text.as_str()?).ok()?;
        let pairs = |k: &str| -> std::collections::BTreeMap<String, String> {
            v[k].as_object().map(|o| o.iter().filter_map(|(a, b)| Some((a.clone(), b.as_str()?.to_string()))).collect()).unwrap_or_default()
        };
        Some((v["o"].as_str()?.to_string(), crate::cookies::OriginStorage { local: pairs("l"), session: pairs("s") }))
    }

    /// 页面上用户看得到、能操作的元素（无障碍树）：(角色, 名称, 状态)
    pub fn ax_elements(&mut self) -> Result<Vec<(String, String, Vec<String>)>> {
        let tree = self.call("Accessibility.getFullAXTree", json!({}))?;
        let mut out = vec![];
        for n in tree["nodes"].as_array().into_iter().flatten() {
            if n["ignored"].as_bool().unwrap_or(false) {
                continue;
            }
            let role = n["role"]["value"].as_str().unwrap_or_default();
            if !SNAPSHOT_ROLES.contains(&role) {
                continue;
            }
            let name = n["name"]["value"].as_str().unwrap_or_default().trim().to_string();
            if name.is_empty() && matches!(role, "img" | "image" | "heading" | "dialog" | "alert" | "status" | "cell" | "gridcell") {
                continue;
            }
            let mut states = vec![];
            for p in n["properties"].as_array().into_iter().flatten() {
                let k = p["name"].as_str().unwrap_or_default();
                let on = p["value"]["value"] == json!(true) || p["value"]["value"] == json!("true");
                match k {
                    "disabled" if on => states.push("禁用".to_string()),
                    "checked" if on => states.push("已勾选".to_string()),
                    "expanded" if on => states.push("已展开".to_string()),
                    "focused" if on => states.push("焦点".to_string()),
                    "required" if on => states.push("必填".to_string()),
                    _ => {}
                }
            }
            out.push((role.to_string(), name, states));
        }
        Ok(out)
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// 无障碍树里按角色 / 名称（或表单标签）找节点
    fn find_ax(&mut self, loc: &Locator) -> Result<Vec<String>> {
        let tree = self.call("Accessibility.getFullAXTree", json!({}))?;
        let exact = loc.exact.unwrap_or(false);
        let nodes = tree["nodes"].as_array().cloned().unwrap_or_default();
        let mut ids: Vec<i64> = vec![];
        for n in &nodes {
            if n["ignored"].as_bool().unwrap_or(false) {
                continue;
            }
            let Some(backend) = n["backendDOMNodeId"].as_i64() else { continue };
            let role = n["role"]["value"].as_str().unwrap_or_default();
            let name = n["name"]["value"].as_str().unwrap_or_default();
            let ok = if let Some(want) = &loc.role {
                role_matches(role, want) && loc.name.as_ref().is_none_or(|w| super::ui::text_matches(name, w, exact))
            } else if let Some(label) = &loc.label {
                FORM_ROLES.contains(&role) && super::ui::text_matches(name, label, exact)
            } else {
                false
            };
            if ok && !ids.contains(&backend) {
                ids.push(backend);
            }
            if ids.len() >= 200 {
                break;
            }
        }
        let mut objs = vec![];
        for id in ids {
            if let Ok(r) = self.call("DOM.resolveNode", json!({ "backendNodeId": id, "objectGroup": GROUP })) {
                if let Some(o) = r["object"]["objectId"].as_str() {
                    objs.push(o.to_string());
                }
            }
        }
        Ok(objs)
    }
}

impl Drop for WebPage<'_> {
    fn drop(&mut self) {
        let ctx = self.context.clone();
        let _ = self.b.cdp.call(None, "Target.disposeBrowserContext", json!({ "browserContextId": ctx }), Duration::from_secs(5));
    }
}

/// 快照列出的角色：能操作的控件 + 标题 / 图片 / 提示等定位锚点
const SNAPSHOT_ROLES: [&str; 24] = [
    "button", "link", "textbox", "searchbox", "checkbox", "radio", "combobox", "listbox", "option", "menuitem", "menuitemcheckbox", "menuitemradio", "tab",
    "switch", "slider", "spinbutton", "heading", "img", "image", "dialog", "alertdialog", "alert", "status", "PopUpButton",
];

/// 表单控件的角色（label 定位只在这些里找）
const FORM_ROLES: [&str; 11] = ["textbox", "searchbox", "combobox", "checkbox", "radio", "spinbutton", "slider", "switch", "listbox", "textarea", "PopUpButton"];

fn role_matches(actual: &str, want: &str) -> bool {
    let want = want.to_ascii_lowercase();
    let actual = actual.to_ascii_lowercase();
    actual == want
        || matches!((want.as_str(), actual.as_str()), ("img", "image") | ("image", "img") | ("textbox", "searchbox") | ("combobox", "popupbutton") | ("cell", "gridcell"))
}

fn dialog_label(kind: &str) -> &'static str {
    match kind {
        "confirm" => "确认框",
        "prompt" => "输入框",
        "beforeunload" => "离开页面提示",
        _ => "提示框",
    }
}

fn str_of(v: &Value, key: &str) -> Result<String> {
    v.get(key).and_then(Value::as_str).map(String::from).ok_or_else(|| anyhow!("浏览器没有返回 {key}"))
}

fn exception_text(e: &Value) -> String {
    e["exception"]["description"].as_str().map(|d| d.lines().next().unwrap_or(d).to_string()).unwrap_or_else(|| e["text"].as_str().unwrap_or("未知错误").to_string())
}

/// 页面内的查找：文字 / 占位文字 / testId / CSS（含 Shadow DOM）
const FIND_JS: &str = r#"function(a){
  const norm = s => (s == null ? '' : String(s)).replace(/\s+/g, ' ').trim();
  const has = (s, w) => norm(s).toLowerCase().includes(norm(w).toLowerCase());
  const m = (s, w) => a.exact ? norm(s) === norm(w) : has(s, w);
  const all = (root, sel) => {
    const out = [...root.querySelectorAll(sel)];
    for (const el of root.querySelectorAll('*')) if (el.shadowRoot) out.push(...all(el.shadowRoot, sel));
    return out;
  };
  if (a.css) return all(document, a.css);
  if (a.testId) return all(document, '[data-testid],[data-test-id],[data-test]').filter(e =>
    e.getAttribute('data-testid') === a.testId || e.getAttribute('data-test-id') === a.testId || e.getAttribute('data-test') === a.testId);
  if (a.placeholder) return all(document, '[placeholder]').filter(e => m(e.getAttribute('placeholder'), a.placeholder));
  const skip = new Set(['SCRIPT', 'STYLE', 'NOSCRIPT', 'TEMPLATE', 'HEAD', 'TITLE', 'META', 'svg']);
  const own = el => (el.tagName === 'INPUT' && /^(button|submit|reset)$/i.test(el.type)) ? el.value : el.textContent;
  const out = [];
  const walk = el => {
    let found = false;
    const kids = [...el.children, ...(el.shadowRoot ? el.shadowRoot.children : [])];
    for (const c of kids) if (!skip.has(c.tagName) && has(own(c), a.text)) found = walk(c) || found;
    if (!found && m(own(el), a.text)) { out.push(el); return true; }
    return found;
  };
  if (document.body && has(document.body.textContent, a.text)) walk(document.body);
  for (const b of all(document, 'input[type=button],input[type=submit],input[type=reset]')) if (m(b.value, a.text) && !out.includes(b)) out.push(b);
  return out;
}"#;

/// 取下标为 i 的元素（无障碍树里可能是文字节点：取其父元素）
const EL: &str = "const E = i => { const e = this[i]; return e && e.nodeType === 1 ? e : (e && e.parentElement); };";

const INFO_JS: &str = r#"function(){ const arr = this;
  const desc = el => { if (!el) return ''; let s = '<' + el.tagName.toLowerCase(); if (el.id) s += ' id="' + el.id + '"';
    const c = (el.getAttribute('class') || '').trim(); if (c) s += ' class="' + c.slice(0, 60) + '"'; return s + '>'; };
  return Array.from(arr).map(n => {
    const el = n && n.nodeType === 1 ? n : (n && n.parentElement);
    if (!el) return null;
    const r = el.getBoundingClientRect(); const st = getComputedStyle(el);
    const visible = r.width > 0 && r.height > 0 && st.visibility !== 'hidden' && st.display !== 'none';
    const enabled = !(el.disabled || el.closest('fieldset[disabled]') || el.getAttribute('aria-disabled') === 'true');
    let checked = null;
    if (el.tagName === 'INPUT' && /^(checkbox|radio)$/i.test(el.type)) checked = el.checked;
    else if (el.hasAttribute('aria-checked')) checked = el.getAttribute('aria-checked') === 'true';
    else if (el.hasAttribute('aria-pressed')) checked = el.getAttribute('aria-pressed') === 'true';
    const isInput = /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName);
    const value = el.tagName === 'SELECT' ? (el.selectedOptions[0] ? el.selectedOptions[0].text.trim() : '') : (isInput ? el.value : null);
    const editable = (el.tagName === 'INPUT' && !/^(button|submit|reset|checkbox|radio|file|image|hidden|range|color)$/i.test(el.type)) || el.tagName === 'TEXTAREA' || el.isContentEditable;
    const text = (el.tagName === 'INPUT' ? (/^(button|submit|reset)$/i.test(el.type) ? el.value : '') : (el.innerText || el.textContent || '')).trim().slice(0, 500);
    const secret = el.tagName === 'INPUT' && el.type === 'password';
    return { x: r.x, y: r.y, w: r.width, h: r.height, visible, enabled, checked, value: secret ? null : value, editable, readonly: !!el.readOnly, text, tag: desc(el), secret };
  });
}"#;

const ACTIONABLE_JS: &str = r#"function(i){ EL
  const el = E(i);
  if (!el || !el.isConnected) return { gone: true };
  el.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
  const desc = el => { if (!el) return '空白处'; let s = '<' + el.tagName.toLowerCase(); if (el.id) s += ' id="' + el.id + '"';
    const c = (el.getAttribute('class') || '').trim(); if (c) s += ' class="' + c.slice(0, 60) + '"'; return s + '>'; };
  return new Promise(res => requestAnimationFrame(() => { const a = el.getBoundingClientRect(); requestAnimationFrame(() => {
    const b = el.getBoundingClientRect();
    const stable = a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height;
    const x = Math.min(Math.max(b.x + b.width / 2, 1), innerWidth - 1), y = Math.min(Math.max(b.y + b.height / 2, 1), innerHeight - 1);
    let hit = document.elementFromPoint(x, y);
    while (hit && hit.shadowRoot) { const h = hit.shadowRoot.elementFromPoint(x, y); if (!h || h === hit) break; hit = h; }
    const ok = !!hit && (hit === el || el.contains(hit) || (hit.tagName === 'LABEL' && hit.control === el) || (el.tagName === 'LABEL' && el.control === hit)
      || (el.getRootNode() instanceof ShadowRoot && el.getRootNode().host.contains(hit)));
    res({ x: b.x, y: b.y, w: b.width, h: b.height, stable, ok, hit: ok ? '' : desc(hit) });
  }); }));
}"#;

const FOCUS_SELECT_JS: &str = r#"function(i){ EL
  const el = E(i); el.focus();
  if (el.isContentEditable) { const s = getSelection(); s.selectAllChildren(el); }
  else if (el.select) { try { el.select(); } catch (e) {} }
  return document.activeElement === el || el.contains(document.activeElement);
}"#;

const VALUE_JS: &str = r#"function(i){ EL const el = E(i); return el.isContentEditable ? el.innerText : el.value; }"#;

const SET_VALUE_JS: &str = r#"function(i, v){ EL
  const el = E(i);
  const proto = el.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, v);
  el.dispatchEvent(new Event('input', { bubbles: true }));
  el.dispatchEvent(new Event('change', { bubbles: true }));
}"#;

const SELECT_JS: &str = r#"function(i, v){ EL
  const el = E(i);
  if (el.tagName !== 'SELECT') return { err: '不是下拉框（' + el.tagName.toLowerCase() + '），点击打开后再点选项' };
  const opts = [...el.options]; const t = o => o.text.replace(/\s+/g, ' ').trim();
  const o = opts.find(o => o.value === v) || opts.find(o => t(o) === v) || opts.find(o => t(o).toLowerCase().includes(v.toLowerCase()));
  if (!o) return { err: '没有选项「' + v + '」，可选：' + opts.map(t).join('、') };
  if (o.disabled) return { err: '选项「' + t(o) + '」是禁用的' };
  el.focus(); el.value = o.value; o.selected = true;
  el.dispatchEvent(new Event('input', { bubbles: true })); el.dispatchEvent(new Event('change', { bubbles: true }));
  return { ok: t(o) };
}"#;

const ATTR_JS: &str = r#"function(i, n){ EL
  const el = E(i); const v = el.getAttribute(n);
  if (v != null) return v;
  if (n in el && el[n] != null && typeof el[n] !== 'object' && typeof el[n] !== 'function') return String(el[n]);
  return null;
}"#;

fn with_el(js: &str) -> String {
    js.replacen("EL", EL, 1)
}

/// 按键写法：Enter / Tab / Escape / ArrowDown / Control+a / Shift+Tab …
pub(crate) fn key_event(spec: &str) -> Result<(u32, String, String, u32, Option<String>)> {
    let parts: Vec<&str> = spec.split('+').map(str::trim).collect();
    let (mods, key) = parts.split_at(parts.len() - 1);
    let mut modifiers = 0;
    for m in mods {
        modifiers |= match m.to_ascii_lowercase().as_str() {
            "alt" | "option" => 1,
            "control" | "ctrl" => 2,
            "meta" | "cmd" | "command" => 4,
            "shift" => 8,
            other => bail!("不认识的修饰键 {other}（Control / Shift / Alt / Meta）"),
        };
    }
    let key = key[0];
    let named: Option<(&str, u32, Option<&str>)> = match key.to_ascii_lowercase().as_str() {
        "enter" => Some(("Enter", 13, Some("\r"))),
        "tab" => Some(("Tab", 9, None)),
        "escape" | "esc" => Some(("Escape", 27, None)),
        "backspace" => Some(("Backspace", 8, None)),
        "delete" => Some(("Delete", 46, None)),
        "space" => Some((" ", 32, Some(" "))),
        "arrowup" | "up" => Some(("ArrowUp", 38, None)),
        "arrowdown" | "down" => Some(("ArrowDown", 40, None)),
        "arrowleft" | "left" => Some(("ArrowLeft", 37, None)),
        "arrowright" | "right" => Some(("ArrowRight", 39, None)),
        "home" => Some(("Home", 36, None)),
        "end" => Some(("End", 35, None)),
        "pageup" => Some(("PageUp", 33, None)),
        "pagedown" => Some(("PageDown", 34, None)),
        _ => None,
    };
    if let Some((k, code, text)) = named {
        let c = if k == " " { "Space".to_string() } else { k.to_string() };
        return Ok((modifiers, k.to_string(), c, code, text.filter(|_| modifiers & 0b0111 == 0).map(String::from)));
    }
    if let Some(n) = key.strip_prefix(['F', 'f']).and_then(|n| n.parse::<u32>().ok()).filter(|n| (1..=12).contains(n)) {
        return Ok((modifiers, format!("F{n}"), format!("F{n}"), 111 + n, None));
    }
    let mut chars = key.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        let upper = c.to_ascii_uppercase();
        let code = if c.is_ascii_alphabetic() { format!("Key{upper}") } else if c.is_ascii_digit() { format!("Digit{c}") } else { String::new() };
        let key_code = if c.is_ascii_alphanumeric() { upper as u32 } else { 0 };
        let text = (modifiers & 0b0111 == 0).then(|| if modifiers & 8 != 0 { upper.to_string() } else { c.to_string() });
        return Ok((modifiers, c.to_string(), code, key_code, text));
    }
    bail!("不认识的按键 {key}（Enter / Tab / Escape / Backspace / ArrowDown / Control+a …）")
}

impl UiDriver for WebPage<'_> {
    fn android(&self) -> bool {
        false
    }

    fn find(&mut self, loc: &Locator) -> Result<Vec<Element>> {
        self.process();
        let _ = self.call("Runtime.releaseObjectGroup", json!({ "objectGroup": GROUP }));
        self.found = None;
        let arr = if loc.role.is_some() || loc.label.is_some() {
            let objs = self.find_ax(loc)?;
            if objs.is_empty() {
                return Ok(vec![]);
            }
            let args: Vec<Value> = objs.iter().map(|o| json!({ "objectId": o })).collect();
            let r = self.call(
                "Runtime.callFunctionOn",
                json!({ "objectId": objs[0], "functionDeclaration": "function(){ return Array.from(arguments); }", "arguments": args, "objectGroup": GROUP }),
            )?;
            r["result"]["objectId"].as_str().map(String::from)
        } else {
            let a = json!({ "text": loc.text, "placeholder": loc.placeholder, "testId": loc.test_id, "css": loc.css, "exact": loc.exact.unwrap_or(false) });
            let r = self.call("Runtime.evaluate", json!({ "expression": format!("({FIND_JS})({a})"), "objectGroup": GROUP, "returnByValue": false }))?;
            if let Some(e) = r.get("exceptionDetails") {
                let text = exception_text(e);
                if loc.css.is_some() {
                    bail!("CSS 选择器 `{}` 写法有误：{text}", loc.css.as_deref().unwrap_or_default());
                }
                bail!("查找元素出错：{text}");
            }
            r["result"]["objectId"].as_str().map(String::from)
        };
        let Some(arr) = arr else { return Ok(vec![]) };
        self.found = Some(arr);
        let infos = self.on_found(INFO_JS, vec![], false)?;
        let mut out = vec![];
        for (i, v) in infos.as_array().cloned().unwrap_or_default().into_iter().enumerate() {
            if v.is_null() {
                continue;
            }
            let f = |k: &str| v[k].as_f64().unwrap_or(0.0);
            out.push(Element {
                handle: i,
                rect: Rect { x: f("x"), y: f("y"), w: f("w"), h: f("h") },
                visible: v["visible"].as_bool().unwrap_or(false),
                enabled: v["enabled"].as_bool().unwrap_or(true),
                checked: v["checked"].as_bool(),
                editable: v["editable"].as_bool().unwrap_or(false),
                readonly: v["readonly"].as_bool().unwrap_or(false),
                text: v["text"].as_str().unwrap_or_default().to_string(),
                value: v["value"].as_str().map(String::from),
                tag: v["tag"].as_str().unwrap_or_default().to_string(),
                secret: v["secret"].as_bool().unwrap_or(false),
            });
        }
        Ok(out)
    }

    fn page(&mut self) -> Result<PageInfo> {
        self.process();
        let v = self.eval("({ url: location.href, title: document.title })")?;
        Ok(PageInfo { location: v["url"].as_str().unwrap_or_default().into(), title: v["title"].as_str().unwrap_or_default().into() })
    }

    fn actionable(&mut self, el: &Element) -> Result<Actionable> {
        let v = self.on_found(&with_el(ACTIONABLE_JS), vec![json!(el.handle)], true)?;
        if v["gone"].as_bool() == Some(true) {
            return Ok(Actionable::NotYet("元素已从页面移除".into()));
        }
        let mut el = el.clone();
        el.rect = Rect { x: v["x"].as_f64().unwrap_or(0.0), y: v["y"].as_f64().unwrap_or(0.0), w: v["w"].as_f64().unwrap_or(0.0), h: v["h"].as_f64().unwrap_or(0.0) };
        if v["stable"].as_bool() != Some(true) {
            return Ok(Actionable::NotYet("位置还在变化（动画中）".into()));
        }
        if v["ok"].as_bool() != Some(true) {
            return Ok(Actionable::NotYet(format!("被 {} 遮挡", v["hit"].as_str().unwrap_or("其他元素"))));
        }
        Ok(Actionable::Ok(el))
    }

    fn click(&mut self, el: &Element) -> Result<()> {
        self.begin_action();
        let (x, y) = self.point_of(el);
        self.mouse("mouseMoved", x, y, "none", 0)?;
        self.mouse("mousePressed", x, y, "left", 1)?;
        self.mouse("mouseReleased", x, y, "left", 1)
    }

    fn long_press(&mut self, el: &Element) -> Result<()> {
        self.begin_action();
        let (x, y) = self.point_of(el);
        self.mouse("mouseMoved", x, y, "none", 0)?;
        self.mouse("mousePressed", x, y, "left", 1)?;
        std::thread::sleep(Duration::from_millis(800));
        self.mouse("mouseReleased", x, y, "left", 1)
    }

    fn hover(&mut self, el: &Element) -> Result<()> {
        self.begin_action();
        let (x, y) = self.point_of(el);
        self.mouse("mouseMoved", x, y, "none", 0)
    }

    fn fill(&mut self, el: &Element, value: &str) -> Result<()> {
        if !el.editable {
            bail!("{} 不是输入框，无法填写", el.tag);
        }
        if el.readonly {
            bail!("输入框是只读的");
        }
        self.begin_action();
        self.on_found(&with_el(FOCUS_SELECT_JS), vec![json!(el.handle)], false)?;
        if value.is_empty() {
            let (m, k, c, code, _) = key_event("Backspace")?;
            for t in ["rawKeyDown", "keyUp"] {
                self.call("Input.dispatchKeyEvent", json!({ "type": t, "modifiers": m, "key": k, "code": c, "windowsVirtualKeyCode": code }))?;
            }
        } else {
            self.call("Input.insertText", json!({ "text": value }))?;
        }
        // 日期 / 数字等特殊输入框不接受文本输入：直接设值并触发 input / change
        let now = self.on_found(&with_el(VALUE_JS), vec![json!(el.handle)], false)?;
        if now.as_str().map(|s| s.trim_end_matches('\n')) != Some(value) {
            self.on_found(&with_el(SET_VALUE_JS), vec![json!(el.handle), json!(value)], false)?;
        }
        Ok(())
    }

    fn select(&mut self, el: &Element, value: &str) -> Result<String> {
        self.begin_action();
        let r = self.on_found(&with_el(SELECT_JS), vec![json!(el.handle), json!(value)], false)?;
        if let Some(e) = r["err"].as_str() {
            bail!("{e}");
        }
        Ok(r["ok"].as_str().unwrap_or(value).to_string())
    }

    fn type_text(&mut self, text: &str) -> Result<()> {
        self.begin_action();
        self.call("Input.insertText", json!({ "text": text }))?;
        Ok(())
    }

    fn press(&mut self, key: &str) -> Result<()> {
        let (modifiers, k, code, vk, text) = key_event(key)?;
        self.begin_action();
        let mut down = json!({ "type": if text.is_some() { "keyDown" } else { "rawKeyDown" }, "modifiers": modifiers, "key": k, "code": code, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk });
        if let Some(t) = &text {
            down["text"] = json!(t);
            down["unmodifiedText"] = json!(t);
        }
        self.call("Input.dispatchKeyEvent", down)?;
        self.call("Input.dispatchKeyEvent", json!({ "type": "keyUp", "modifiers": modifiers, "key": k, "code": code, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk }))?;
        Ok(())
    }

    fn swipe(&mut self, _direction: &str) -> Result<()> {
        bail!("Web 用例不支持 swipe（Android 专用）")
    }

    fn back(&mut self) -> Result<()> {
        let h = self.call("Page.getNavigationHistory", json!({}))?;
        let i = h["currentIndex"].as_i64().unwrap_or(0);
        if i <= 0 {
            bail!("已经是第一页，无法返回");
        }
        let id = h["entries"][(i - 1) as usize]["id"].clone();
        self.begin_action();
        self.loading = true;
        self.call("Page.navigateToHistoryEntry", json!({ "entryId": id }))?;
        Ok(())
    }

    fn goto(&mut self, url: &str) -> Result<()> {
        let target = if url.contains("://") || url.starts_with("about:") || url.starts_with("data:") {
            url.to_string()
        } else {
            let base = self.page().map(|p| p.location).unwrap_or_default();
            if base.is_empty() || base.starts_with("about:") {
                bail!("{url} 是相对地址，但当前还没有打开任何页面：写完整地址");
            }
            self.eval(&format!("new URL({}, location.href).href", json!(url)))?.as_str().unwrap_or(url).to_string()
        };
        self.begin_action();
        self.doc_status = 0;
        self.doc_headers.clear();
        self.loading = true;
        let r = self.call("Page.navigate", json!({ "url": target }))?;
        if let Some(e) = r.get("errorText").and_then(Value::as_str).filter(|e| !e.is_empty()) {
            self.loading = false;
            let remote = if self.b.remote.is_some() { remote_hint(&target) } else { "" };
            bail!("打开 {target} 失败：{e}{}{remote}", explain_net_error(e));
        }
        if r.get("loaderId").is_none() {
            // 同一文档内跳转（只改了 #hash）：没有加载过程
            self.loading = false;
        }
        let t = self.nav_timeout;
        let loaded = self.wait_loaded(t).with_context(|| format!("打开 {target}"));
        if let Some(id) = self.storage_script.take() {
            let _ = self.call("Page.removeScriptToEvaluateOnNewDocument", json!({ "identifier": id }));
        }
        loaded
    }

    fn launch(&mut self, _clear: bool) -> Result<()> {
        bail!("Web 用例不支持 launch（Android 专用）")
    }

    fn stop(&mut self) -> Result<()> {
        bail!("Web 用例不支持 stop（Android 专用）")
    }

    fn settle(&mut self, timeout: Duration) {
        // 动作可能触发跳转：等加载完成（请求了跳转却迟迟没开始加载的，如下载文件，2 秒后不再等）
        let _ = self.b.cdp.pump(Duration::from_millis(120));
        self.process();
        let started = Instant::now();
        while (self.loading || self.nav_pending) && started.elapsed() < timeout {
            if self.nav_pending && !self.loading && started.elapsed() > Duration::from_secs(2) {
                break;
            }
            if self.b.cdp.pump(Duration::from_millis(50)).is_err() {
                break;
            }
            self.process();
        }
        self.loading = false;
        self.nav_pending = false;
        let _ = self.call(
            "Runtime.evaluate",
            json!({ "expression": "new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))", "awaitPromise": true, "timeout": 1000 }),
        );
        self.process();
    }

    fn screenshot(&mut self) -> Result<Shot> {
        let r = self.call("Page.captureScreenshot", json!({ "format": "png" }))?;
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD.decode(r["data"].as_str().unwrap_or_default()).context("截图数据无效")?;
        Ok(Shot { bytes, scale: 1.0 })
    }

    fn attr(&mut self, el: &Element, name: &str) -> Result<Option<String>> {
        let v = self.on_found(&with_el(ATTR_JS), vec![json!(el.handle), json!(name)], false)?;
        Ok(v.as_str().map(String::from))
    }

    fn failure_scene(&mut self, dir: &Path) -> Vec<Artifact> {
        self.process();
        let mut out = vec![];
        let mut save = |label: &str, file: &str, content: String| {
            if !content.is_empty() && crate::fsx::write(dir.join(file), content.as_bytes()).is_ok() {
                out.push(Artifact { label: label.into(), file: file.into() });
            }
        };
        let html = self.eval("document.documentElement ? document.documentElement.outerHTML : ''").ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
        save("页面 HTML", "page.html", html.chars().take(2_000_000).collect());
        let outline = self.call("Accessibility.getFullAXTree", json!({})).map(|t| ax_outline(&t)).unwrap_or_default();
        save("无障碍树", "accessibility.txt", outline);
        save("控制台", "console.txt", self.console.join("\n"));
        save("网络请求", "network.txt", self.network.join("\n"));
        out
    }

    fn environment(&mut self) -> Vec<(String, String)> {
        let mut env = vec![
            ("浏览器".into(), self.b.product.replace('/', " ")),
            ("模式".into(), self.b.mode()),
            ("视口".into(), format!("{}×{}", self.viewport.0, self.viewport.1)),
            ("User-Agent".into(), self.b.user_agent.clone()),
            ("系统".into(), format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)),
        ];
        if let Some(p) = self.b.proxy_note() {
            env.insert(2, ("代理".into(), p));
        }
        env
    }

    fn take_notes(&mut self) -> Vec<String> {
        self.process();
        std::mem::take(&mut self.notes)
    }
}

fn explain_net_error(e: &str) -> &'static str {
    match e {
        "net::ERR_CONNECTION_REFUSED" => "（服务没有启动，或端口不对）",
        "net::ERR_NAME_NOT_RESOLVED" => "（域名解析失败）",
        "net::ERR_CERT_AUTHORITY_INVALID" | "net::ERR_CERT_COMMON_NAME_INVALID" | "net::ERR_CERT_DATE_INVALID" => "（证书不受信任）",
        "net::ERR_CONNECTION_TIMED_OUT" | "net::ERR_TIMED_OUT" => "（连接超时）",
        _ => "",
    }
}

/// 远程浏览器访问不到本机地址：给出改法
fn remote_hint(target: &str) -> &'static str {
    let host = target.split_once("://").map(|(_, r)| r).unwrap_or(target).split(['/', '?', '#']).next().unwrap_or_default();
    let host = host.rsplit_once('@').map(|(_, h)| h).unwrap_or(host);
    let host = if host.starts_with('[') { host.split(']').next().unwrap_or(host).trim_start_matches('[') } else { host.split(':').next().unwrap_or(host) };
    if target.starts_with("file:") {
        "。远程浏览器打不开本机文件：把页面放到它能访问的地址上"
    } else if host == "localhost" || host == "::1" || host.starts_with("127.") || host == "0.0.0.0" {
        "。远程浏览器里的 localhost 是远程那台机器：改用本机的局域网地址（浏览器在本机 Docker 里时用 host.docker.internal）"
    } else {
        ""
    }
}

/// 无障碍树的缩进文本：`button "登录"`，只列有意义的节点
pub fn ax_outline(tree: &Value) -> String {
    let nodes = tree["nodes"].as_array().cloned().unwrap_or_default();
    let by_id: std::collections::HashMap<&str, &Value> = nodes.iter().filter_map(|n| n["nodeId"].as_str().map(|id| (id, n))).collect();
    let mut out = String::new();
    fn walk(id: &str, depth: usize, by_id: &std::collections::HashMap<&str, &Value>, out: &mut String) {
        let Some(n) = by_id.get(id) else { return };
        let role = n["role"]["value"].as_str().unwrap_or_default();
        let name = n["name"]["value"].as_str().unwrap_or_default();
        let ignored = n["ignored"].as_bool().unwrap_or(false);
        let shown = !ignored && !matches!(role, "generic" | "none" | "InlineTextBox" | "LineBreak" | "") && !(role == "StaticText" && name.trim().is_empty());
        let next = if shown {
            let mut line = format!("{}{role}", "  ".repeat(depth));
            if !name.is_empty() {
                line.push_str(&format!(" \"{}\"", name.chars().take(120).collect::<String>().replace('\n', " ")));
            }
            for p in n["properties"].as_array().into_iter().flatten() {
                let k = p["name"].as_str().unwrap_or_default();
                if matches!(k, "disabled" | "checked" | "expanded" | "selected" | "focused" | "required" | "invalid") && p["value"]["value"] != json!(false) {
                    line.push_str(&format!(" [{k}]"));
                }
            }
            out.push_str(&line);
            out.push('\n');
            depth + 1
        } else {
            depth
        };
        for c in n["childIds"].as_array().into_iter().flatten() {
            if let Some(c) = c.as_str() {
                walk(c, next, by_id, out);
            }
        }
    }
    if let Some(root) = nodes.first().and_then(|n| n["nodeId"].as_str()) {
        walk(root, 0, &by_id, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_parse() {
        let (m, k, c, code, text) = key_event("Enter").unwrap();
        assert_eq!((m, k.as_str(), c.as_str(), code, text.as_deref()), (0, "Enter", "Enter", 13, Some("\r")));
        let (m, k, c, code, text) = key_event("Control+a").unwrap();
        assert_eq!((m, k.as_str(), c.as_str(), code, text), (2, "a", "KeyA", 65, None));
        assert_eq!(key_event("Shift+Tab").unwrap().0, 8);
        assert!(key_event("Hyper+x").is_err());
        assert!(key_event("Foo").is_err());
    }

    #[test]
    fn outline_lists_meaningful_nodes() {
        let tree = json!({ "nodes": [
            { "nodeId": "1", "role": { "value": "RootWebArea" }, "name": { "value": "登录" }, "childIds": ["2"] },
            { "nodeId": "2", "role": { "value": "generic" }, "childIds": ["3", "4"] },
            { "nodeId": "3", "role": { "value": "button" }, "name": { "value": "登录" }, "properties": [{ "name": "disabled", "value": { "value": true } }] },
            { "nodeId": "4", "role": { "value": "textbox" }, "name": { "value": "手机号" } }
        ]});
        assert_eq!(ax_outline(&tree), "RootWebArea \"登录\"\n  button \"登录\" [disabled]\n  textbox \"手机号\"\n");
    }
}
