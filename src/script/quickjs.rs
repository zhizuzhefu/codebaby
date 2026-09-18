use super::{ReqData, RespData, ScriptHost, ScriptOutcome, TestOutcome};
use anyhow::{anyhow, Result};
use base64::Engine as _;
use rquickjs::{Context, Ctx, Function, Runtime};
use std::collections::BTreeMap;

/// QuickJS 沙箱实现。
///
/// 交互约定（刻意极简，规避 host 绑定复杂度）：
/// - 数据注入：eval 一段 JSON 字面量初始化脚本（serde_json 转义保证安全）
/// - 数据提取：JSON.stringify 后回传 Rust 解析
/// - host 函数：仅无状态 crypto 工具
/// - 每次执行新建 Runtime，杜绝跨用例状态泄漏
#[derive(Debug, Default, Clone, Copy)]
pub struct QuickJsHost;

/// wrapper 模板中用户脚本的首行行号（用于 stack 行号修正）：
/// L1: (function() {
/// L2: try {
/// L3:   (function() {
/// L4:   <user script 第一行>
const USER_SCRIPT_LINE_OFFSET: usize = 3;

impl QuickJsHost {
    pub fn new() -> Self {
        Self
    }

    fn with_ctx<T>(&self, f: impl for<'js> FnOnce(&Ctx<'js>) -> Result<T>) -> Result<T> {
        let rt = Runtime::new().map_err(|e| anyhow!("QuickJS Runtime 创建失败: {e}"))?;
        let ctx = Context::full(&rt).map_err(|e| anyhow!("QuickJS Context 创建失败: {e}"))?;
        ctx.with(|ctx| f(&ctx))
    }

    /// 执行用户脚本（pre/post）：
    /// JS 侧 try/catch 捕获异常并序列化 {name, message, stack} 带回，
    /// 彻底透出 QuickJS 异常细节（Rust 侧 rquickjs::Error 对 JS 异常只有一句黑盒描述）
    fn eval_user(ctx: &Ctx<'_>, script: &str) -> Option<String> {
        let wrapped = format!(
            "(function() {{\ntry {{\n(function() {{\n{script}\n}})();\nreturn '';\n}} \
             catch (e) {{\nreturn JSON.stringify({{name: (e && e.name) || 'Error', \
             message: (e && e.message) || String(e), \
             stack: e && e.stack ? String(e.stack) : ''}});\n}}\n}})()"
        );
        match ctx.eval::<String, _>(wrapped.as_str()) {
            Ok(s) if s.is_empty() => None,
            Ok(s) => Some(pretty_js_error(&s, USER_SCRIPT_LINE_OFFSET)),
            Err(e) => Some(format!("语法错误: {e}")),
        }
    }
}

/// 解析 JS 侧带回的异常 JSON → 可读文本；stack 行号修正为用户脚本相对行号
fn pretty_js_error(raw: &str, line_offset: usize) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return format!("未知异常: {raw}");
    };
    let name = v["name"].as_str().unwrap_or("Error");
    let message = v["message"].as_str().unwrap_or_default();
    let stack = v["stack"].as_str().unwrap_or_default();
    let fixed_stack = fix_stack_lines(stack, line_offset);
    let mut out = format!("{name}: {message}");
    if !fixed_stack.is_empty() {
        out.push_str(&format!("\n  {fixed_stack}"));
    }
    out
}

/// QuickJS stack 形如 "    at fn (eval_script:5)" / "    at eval_script:5"
/// 只保留最内层抛错帧（后续帧为 wrapper 噪声），行号减去偏移为用户脚本真实行号
fn fix_stack_lines(stack: &str, offset: usize) -> String {
    stack
        .lines()
        .find(|l| l.trim_start().starts_with("at "))
        .map(|l| {
            let mut out = String::new();
            let mut rest = l;
            while let Some(pos) = rest.find(':') {
                let after = &rest[pos + 1..];
                let digits_end = after
                    .find(|c: char| !c.is_ascii_digit())
                    .unwrap_or(after.len());
                let (num, tail) = after.split_at(digits_end);
                out.push_str(&rest[..pos + 1]);
                if let Ok(n) = num.parse::<usize>() {
                    if n > offset {
                        out.push_str(&n.saturating_sub(offset).to_string());
                    } else {
                        out.push_str(num);
                    }
                } else {
                    out.push_str(num);
                }
                rest = tail;
            }
            out.push_str(rest);
            out
        })
        .unwrap_or_default()
}

/// 安装公共设施：__logs / log / crypto / http
fn install_common(ctx: &Ctx<'_>) -> Result<()> {
    ctx.eval::<(), _>(
        r#"globalThis.__logs = [];
            globalThis.log = (...a) => __logs.push(a.map(x => { try { return typeof x === 'object' ? JSON.stringify(x) : String(x) } catch(e) { return String(x) } }).join(' '));"#,
    )
    .map_err(|e| anyhow!("初始化 log 失败: {e}"))?;
    let g = ctx.globals();
    g.set("crypto", crypto_module(ctx)?)
        .map_err(|e| anyhow!("安装 crypto 失败: {e}"))?;

    // http.request：同步 curl（参数/返回均 JSON 字符串，规避 host 绑定复杂度）
    let http_fn = Function::new(ctx.clone(), |opts_json: String| -> String {
        do_curl(&opts_json)
    })
    .map_err(|e| anyhow!("注册 __httpRequest 失败: {e}"))?;
    g.set("__httpRequest", http_fn)
        .map_err(|e| anyhow!("安装 __httpRequest 失败: {e}"))?;
    ctx.eval::<(), _>(
        r#"globalThis.http = {
             request(o) {
               const r = JSON.parse(__httpRequest(JSON.stringify(o || {})));
               if (r.error) throw new Error('http: ' + r.error);
               return r;
             },
             get(url, o)   { return http.request({ ...(o || {}), method: 'GET',  url }); },
             post(url, o)  { return http.request({ ...(o || {}), method: 'POST', url }); },
           };"#,
    )
    .map_err(|e| anyhow!("安装 http 失败: {e}"))?;
    Ok(())
}

/// Rust 侧同步执行一次 curl（脚本级 http.request 后端）
fn do_curl(opts_json: &str) -> String {
    use crate::engine::{CurlTransport, ResolvedRequest, Transport};
    let Ok(v) = serde_json::from_str::<serde_json::Value>(opts_json) else {
        return r#"{"error":"bad opts"}"#.into();
    };
    let req = ResolvedRequest {
        method: v["method"].as_str().unwrap_or("GET").to_uppercase(),
        url: v["url"].as_str().unwrap_or_default().to_string(),
        headers: v["headers"]
            .as_object()
            .map(|m| m.iter().map(|(k, x)| (k.clone(), x.as_str().unwrap_or_default().to_string())).collect())
            .unwrap_or_default(),
        body: v["body"].as_str().filter(|s| !s.is_empty()).map(String::from),
        timeout_ms: v["timeoutMs"].as_u64().unwrap_or(10_000),
        retry: v["retry"].as_u64().unwrap_or(0) as u32,
        insecure: v["insecure"].as_bool().unwrap_or(false),
        follow_redirects: v["followRedirects"].as_bool().unwrap_or(false),
    };
    let transport = CurlTransport::new();
    let resp = match transport.compile(&req).and_then(|inv| transport.run(&inv)) {
        Ok(r) => r,
        Err(e) => return serde_json::json!({ "error": format!("{e:#}") }).to_string(),
    };
    if let Some(e) = &resp.transport_error {
        return serde_json::json!({ "error": e }).to_string();
    }
    let headers: BTreeMap<String, String> = resp
        .headers
        .iter()
        .map(|(k, v)| (k.to_lowercase(), v.clone()))
        .collect();
    let body: String = resp.body_text().chars().take(200_000).collect();
    serde_json::json!({
        "status": resp.status,
        "timeMs": resp.time_ms,
        "headers": headers,
        "body": body,
    })
    .to_string()
}

fn crypto_module<'js>(ctx: &Ctx<'js>) -> Result<rquickjs::Object<'js>> {
    let m = rquickjs::Object::new(ctx.clone())
        .map_err(|e| anyhow!("创建 crypto 对象失败: {e}"))?;
    macro_rules! reg {
        ($name:expr, $f:expr, $what:expr) => {
            m.set($name, Function::new(ctx.clone(), $f))
                .map_err(|e| anyhow!("注册 crypto.{} 失败: {e}", $what))?;
        };
    }
    reg!(
        "md5",
        |s: String| -> String {
            use md5::Digest;
            let mut h = md5::Md5::new();
            h.update(s.as_bytes());
            hex(&h.finalize())
        },
        "md5"
    );
    reg!(
        "sha256",
        |s: String| -> String {
            use sha2::Digest;
            let mut h = sha2::Sha256::new();
            h.update(s.as_bytes());
            hex(&h.finalize())
        },
        "sha256"
    );
    reg!(
        "hmacSha256",
        |key: String, msg: String| -> String {
            use hmac::Mac;
            let mut mac = <hmac::Hmac<sha2::Sha256>>::new_from_slice(key.as_bytes())
                .expect("hmac key");
            mac.update(msg.as_bytes());
            hex(&mac.finalize().into_bytes())
        },
        "hmacSha256"
    );
    reg!(
        "base64Encode",
        |s: String| -> String {
            base64::engine::general_purpose::STANDARD.encode(s.as_bytes())
        },
        "base64Encode"
    );
    reg!(
        "base64Decode",
        |s: String| -> String {
            String::from_utf8_lossy(
                &base64::engine::general_purpose::STANDARD
                    .decode(s.as_bytes())
                    .unwrap_or_default(),
            )
            .into_owned()
        },
        "base64Decode"
    );
    Ok(m)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 注入 vars / env
fn inject_scope(ctx: &Ctx<'_>, vars: &BTreeMap<String, String>, env: &BTreeMap<String, String>) -> Result<()> {
    let vars_js = serde_json::to_string(vars)?;
    let env_js = serde_json::to_string(env)?;
    ctx.eval::<(), _>(format!(
        "globalThis.vars = {vars_js};
         globalThis.env = {env_js};"
    ))
    .map_err(|e| anyhow!("注入作用域失败: {e}"))?;
    Ok(())
}

/// 注入 req（含 setJson 辅助）
fn inject_req(ctx: &Ctx<'_>, req: &ReqData) -> Result<()> {
    let literal = serde_json::to_string(&serde_json::json!({
        "method": req.method,
        "url": req.url,
        "headers": req.headers,
        "body": req.body,
    }))?;
    ctx.eval::<(), _>(format!(
        "globalThis.req = {literal};
         globalThis.req.setJson = function(o) {{ this.body = JSON.stringify(o); }};"
    ))
    .map_err(|e| anyhow!("注入 req 失败: {e}"))?;
    Ok(())
}

/// 注入 res（text()/json() 基于 __body）
fn inject_resp(ctx: &Ctx<'_>, resp: &RespData) -> Result<()> {
    let headers: BTreeMap<String, String> = resp
        .headers
        .iter()
        .map(|(k, v)| (k.to_lowercase(), v.clone()))
        .collect();
    let literal = serde_json::to_string(&serde_json::json!({
        "status": resp.status,
        "timeMs": resp.time_ms,
        "size": resp.size,
        "headers": headers,
        "__body": resp.body,
    }))?;
    ctx.eval::<(), _>(format!(
        r#"globalThis.res = {literal};
           globalThis.res.text = function() {{ return this.__body; }};
           globalThis.res.json = function(p) {{
             const v = JSON.parse(this.__body);
             if (p === undefined) return v;
             return String(p).split('.').reduce((acc, k) => (acc == null ? undefined : acc[k]), v);
           }};
           try {{ globalThis.data = JSON.parse(res.__body); }} catch (e) {{ globalThis.data = undefined; }}"#
    ))
    .map_err(|e| anyhow!("注入 res 失败: {e}"))?;
    Ok(())
}

fn extract(ctx: &Ctx<'_>, take_req: bool) -> Result<ScriptOutcome> {
    let mut out = ScriptOutcome::default();
    if take_req {
        let raw: String = ctx
            .eval("JSON.stringify({method: req.method, url: req.url, headers: req.headers, body: req.body ?? null})")
            .map_err(|e| anyhow!("提取 req 失败: {e}"))?;
        let v: serde_json::Value = serde_json::from_str(&raw)?;
        out.req = Some(ReqData {
            method: v["method"].as_str().unwrap_or("GET").to_string(),
            url: v["url"].as_str().unwrap_or_default().to_string(),
            headers: v["headers"]
                .as_object()
                .map(|o| {
                    o.iter()
                        .map(|(k, x)| (k.clone(), x.as_str().unwrap_or_default().to_string()))
                        .collect()
                })
                .unwrap_or_default(),
            body: v["body"].as_str().map(String::from),
        });
    }
    let vars_raw: String = ctx
        .eval("JSON.stringify(vars)")
        .map_err(|e| anyhow!("提取 vars 失败: {e}"))?;
    out.vars = parse_string_map(&vars_raw)?;
    let logs: String = ctx
        .eval("JSON.stringify(__logs)")
        .map_err(|e| anyhow!("提取 logs 失败: {e}"))?;
    out.logs = serde_json::from_str(&logs)?;
    Ok(out)
}

/// vars JSON → BTreeMap<String,String>（数字/布尔转字符串，对象保留 JSON 形态）
fn parse_string_map(raw: &str) -> Result<BTreeMap<String, String>> {
    let v: serde_json::Value = serde_json::from_str(raw)?;
    let mut out = BTreeMap::new();
    if let serde_json::Value::Object(m) = v {
        for (k, val) in m {
            let s = match val {
                serde_json::Value::String(s) => s,
                serde_json::Value::Null => String::new(),
                other => other.to_string(),
            };
            out.insert(k, s);
        }
    }
    Ok(out)
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        let mut end = n;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

impl ScriptHost for QuickJsHost {
    fn run_pre(
        &self,
        req: ReqData,
        vars: &BTreeMap<String, String>,
        env: &BTreeMap<String, String>,
        script: &str,
    ) -> Result<ScriptOutcome> {
        self.with_ctx(|ctx| {
            install_common(ctx)?;
            inject_scope(ctx, vars, env)?;
            inject_req(ctx, &req)?;
            let err = Self::eval_user(ctx, script);
            let mut out = extract(ctx, true)?;
            out.error = err;
            Ok(out)
        })
    }

    fn run_post(
        &self,
        resp: &RespData,
        vars: &BTreeMap<String, String>,
        env: &BTreeMap<String, String>,
        script: &str,
    ) -> Result<ScriptOutcome> {
        self.with_ctx(|ctx| {
            install_common(ctx)?;
            inject_scope(ctx, vars, env)?;
            inject_resp(ctx, resp)?;
            let err = Self::eval_user(ctx, script);
            let mut out = extract(ctx, false)?;
            out.error = err;
            Ok(out)
        })
    }

    fn run_scoped(
        &self,
        vars: &BTreeMap<String, String>,
        env: &BTreeMap<String, String>,
        script: &str,
    ) -> Result<ScriptOutcome> {
        self.with_ctx(|ctx| {
            install_common(ctx)?;
            inject_scope(ctx, vars, env)?;
            let err = Self::eval_user(ctx, script);
            let mut out = extract(ctx, false)?;
            out.error = err;
            Ok(out)
        })
    }

    fn eval_tests(
        &self,
        resp: &RespData,
        vars: &BTreeMap<String, String>,
        env: &BTreeMap<String, String>,
        tests: &[String],
    ) -> Result<Vec<TestOutcome>> {
        self.with_ctx(|ctx| {
            install_common(ctx)?;
            inject_scope(ctx, vars, env)?;
            inject_resp(ctx, resp)?;
            let mut results = vec![];
            for expr in tests {
                // 单行 try/catch wrapper：'R:<bool>' 正常求值 / 'E:<json>' 执行错误
                let wrapped = format!(
                    "(function() {{ try {{ return 'R:' + Boolean({expr}); }} \
                     catch (e) {{ var m = (e && e.message || '').match(/property '([^']+)' of/); \
                     return 'E:' + JSON.stringify({{name: (e && e.name) || 'Error', \
                     message: (e && e.message) || String(e), path: m ? m[1] : ''}}); }} }})()"
                );
                let out: String = ctx
                    .eval(wrapped.as_str())
                    .map_err(|e| anyhow!("tests 编译失败: {e}"))?;
                if let Some(v) = out.strip_prefix("R:") {
                    let pass = v == "true";
                    results.push(TestOutcome {
                        expr: expr.clone(),
                        pass,
                        detail: if pass {
                            None
                        } else {
                            Some(format!(
                                "求值为 false（status={} body 前 200 字: {}）",
                                resp.status,
                                truncate(&resp.body, 200)
                            ))
                        },
                    });
                } else if let Some(errj) = out.strip_prefix("E:") {
                    // 执行错误 ≠ 断言失败：表达式本身抛异常（多为 null/undefined 链式取值），
                    // 属配置问题，给出精确定位与修复提示
                    let name = serde_json::from_str::<serde_json::Value>(errj)
                        .ok()
                        .and_then(|v| v["name"].as_str().map(String::from))
                        .unwrap_or_else(|| "Error".into());
                    let msg = serde_json::from_str::<serde_json::Value>(errj)
                        .ok()
                        .and_then(|v| v["message"].as_str().map(String::from))
                        .unwrap_or_default();
                    let path = serde_json::from_str::<serde_json::Value>(errj)
                        .ok()
                        .and_then(|v| v["path"].as_str().map(String::from))
                        .unwrap_or_default();
                    let mut detail = format!("执行错误: {name}: {msg}");
                    if !path.is_empty() {
                        detail.push_str(&format!("（访问了 null/undefined 的 `{path}` 字段）"));
                    }
                    detail.push_str("｜提示: 用 data.a.b（=res.json() 快照）逐级判断，或 res.json('a.b.c') 安全取值");
                    results.push(TestOutcome {
                        expr: expr.clone(),
                        pass: false,
                        detail: Some(detail),
                    });
                } else {
                    results.push(TestOutcome {
                        expr: expr.clone(),
                        pass: false,
                        detail: Some(format!("内部协议异常: {out}")),
                    });
                }
            }
            Ok(results)
        })
    }

    fn compile_check(&self, script: &str) -> Result<()> {
        self.with_ctx(|ctx| {
            let literal = serde_json::to_string(script)?;
            ctx.eval::<rquickjs::Value, _>(format!("new Function({literal})"))
                .map_err(|e| anyhow!("脚本语法错误: {e}"))?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resp() -> RespData {
        RespData {
            status: 201,
            time_ms: 45,
            size: 58,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: r#"{"data":{"id":42,"sku":"A1"}}"#.into(),
        }
    }

    #[test]
    fn pre_modifies_req() {
        let host = QuickJsHost::new();
        let req = ReqData {
            method: "POST".into(),
            url: "http://x".into(),
            headers: BTreeMap::new(),
            body: Some("{}".into()),
        };
        let out = host
            .run_pre(
                req,
                &BTreeMap::new(),
                &BTreeMap::new(),
                "req.headers['X-A'] = '1'; req.setJson({b: 2}); vars.k = 'v'; log('hi')",
            )
            .unwrap();
        assert!(out.error.is_none());
        let req = out.req.unwrap();
        assert_eq!(req.headers.get("X-A").unwrap(), "1");
        assert_eq!(req.body.unwrap(), r#"{"b":2}"#);
        assert_eq!(out.vars.get("k").unwrap(), "v");
        assert_eq!(out.logs, vec!["hi".to_string()]);
    }

    #[test]
    fn post_captures_var() {
        let host = QuickJsHost::new();
        let out = host
            .run_post(&resp(), &BTreeMap::new(), &BTreeMap::new(), "vars.orderId = res.json().data.id")
            .unwrap();
        assert_eq!(out.vars.get("orderId").unwrap(), "42");
    }

    #[test]
    fn tests_eval() {
        let host = QuickJsHost::new();
        let out = host
            .eval_tests(
                &resp(),
                &BTreeMap::new(),
                &BTreeMap::new(),
                &[
                    "res.status === 201".into(),
                    "res.json().data.sku === 'B9'".into(),
                    "res.json().nope.id > 0".into(),
                ],
            )
            .unwrap();
        assert!(out[0].pass);
        assert!(!out[1].pass);
        assert!(!out[2].pass);
        assert!(out[2].detail.as_deref().unwrap().contains("执行错误"));
        assert!(out[2].detail.as_deref().unwrap().contains("安全取值"));
    }

    #[test]
    fn res_json_path_safe_access() {
        let host = QuickJsHost::new();
        let out = host
            .eval_tests(
                &resp(),
                &BTreeMap::new(),
                &BTreeMap::new(),
                &[
                    "res.json('data.sku') === 'A1'".into(),
                    "res.json('data.nope.deeper') === undefined".into(),
                ],
            )
            .unwrap();
        assert!(out[0].pass);
        assert!(out[1].pass);
    }

    #[test]
    fn crypto_works() {
        let host = QuickJsHost::new();
        let out = host
            .run_pre(
                ReqData {
                    method: "GET".into(),
                    url: "u".into(),
                    headers: BTreeMap::new(),
                    body: None,
                },
                &BTreeMap::new(),
                &BTreeMap::new(),
                "vars.h = crypto.md5('abc'); vars.b = crypto.base64Encode('hi')",
            )
            .unwrap();
        assert_eq!(out.vars.get("h").unwrap(), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(out.vars.get("b").unwrap(), "aGk=");
    }

    #[test]
    fn compile_check_catches_syntax() {
        let host = QuickJsHost::new();
        assert!(host.compile_check("vars.a = 1").is_ok());
        assert!(host.compile_check("vars.a = = 1").is_err());
    }

    #[test]
    fn pre_error_carries_details_and_line() {
        let host = QuickJsHost::new();
        let out = host
            .run_pre(
                ReqData { method: "GET".into(), url: "u".into(), headers: BTreeMap::new(), body: None },
                &BTreeMap::new(),
                &BTreeMap::new(),
                "vars.ok = 1\nconst x = null\nx.field = 2",
            )
            .unwrap();
        let err = out.error.expect("应有错误");
        assert!(err.contains("TypeError"), "{err}");
        assert!(err.contains("null"), "{err}");
        // stack 行号修正到用户脚本（第 3 行），不应显示 wrapper 偏移行号 6
        assert!(err.contains(":3"), "行号应修正为 3: {err}");
    }

    #[test]
    fn tests_exec_error_names_field() {
        let host = QuickJsHost::new();
        let mut resp = resp();
        resp.body = r#"{"code":1,"data":null}"#.into();
        let out = host
            .eval_tests(
                &resp,
                &BTreeMap::new(),
                &BTreeMap::new(),
                &["data.results.length > 0".into()],
            )
            .unwrap();
        assert!(!out[0].pass);
        let d = out[0].detail.as_deref().unwrap();
        assert!(d.contains("执行错误"), "{d}");
        assert!(d.contains("`length`"), "应指出抛错字段名: {d}");
    }

    #[test]
    fn data_snapshot_available() {
        let host = QuickJsHost::new();
        let out = host
            .eval_tests(
                &resp(),
                &BTreeMap::new(),
                &BTreeMap::new(),
                &[
                    "data.data.sku === 'A1'".into(),
                    "data.nope === undefined".into(),
                    "res.json('data.nope.deeper') === undefined".into(),
                ],
            )
            .unwrap();
        assert!(out[0].pass);
        assert!(out[1].pass);
        assert!(out[2].pass);
    }
}
