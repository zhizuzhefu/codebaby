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

impl QuickJsHost {
    pub fn new() -> Self {
        Self
    }

    fn with_ctx<T>(&self, f: impl for<'js> FnOnce(&Ctx<'js>) -> Result<T>) -> Result<T> {
        let rt = Runtime::new().map_err(|e| anyhow!("QuickJS Runtime 创建失败: {e}"))?;
        let ctx = Context::full(&rt).map_err(|e| anyhow!("QuickJS Context 创建失败: {e}"))?;
        ctx.with(|ctx| f(&ctx))
    }

    /// 执行用户脚本，异常转 error 字段
    fn eval_user(ctx: &Ctx<'_>, script: &str) -> ScriptOutcome {
        match ctx.eval::<rquickjs::Value, _>(script) {
            Ok(_) => ScriptOutcome::default(),
            Err(e) => ScriptOutcome {
                error: Some(format!("{e}")),
                ..Default::default()
            },
        }
    }
}

/// 安装公共设施：__logs / log / crypto
fn install_common<'js>(ctx: &Ctx<'js>) -> Result<()> {
    ctx.eval::<(), _>(
        r#"globalThis.__logs = [];
            globalThis.log = (...a) => __logs.push(a.map(x => { try { return typeof x === 'object' ? JSON.stringify(x) : String(x) } catch(e) { return String(x) } }).join(' '));"#,
    )
    .map_err(|e| anyhow!("初始化 log 失败: {e}"))?;
    let g = ctx.globals();
    g.set("crypto", crypto_module(ctx)?)
        .map_err(|e| anyhow!("安装 crypto 失败: {e}"))?;
    Ok(())
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
        "globalThis.res = {literal};
         globalThis.res.text = function() {{ return this.__body; }};
         globalThis.res.json = function() {{ return JSON.parse(this.__body); }};"
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
            let mut err = Self::eval_user(ctx, script);
            let mut out = extract(ctx, true)?;
            if err.error.is_some() {
                out.error = err.error.take();
            }
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
            let mut err = Self::eval_user(ctx, script);
            let mut out = extract(ctx, false)?;
            if err.error.is_some() {
                out.error = err.error.take();
            }
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
                let wrapped = format!("Boolean({expr})");
                match ctx.eval::<bool, _>(wrapped.as_str()) {
                    Ok(v) => results.push(TestOutcome {
                        expr: expr.clone(),
                        pass: v,
                        detail: if v {
                            None
                        } else {
                            Some(format!(
                                "求值为 false（status={} body 前 200 字: {}）",
                                resp.status,
                                truncate(&resp.body, 200)
                            ))
                        },
                    }),
                    Err(e) => results.push(TestOutcome {
                        expr: expr.clone(),
                        pass: false,
                        detail: Some(format!("执行错误: {e}")),
                    }),
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
}
