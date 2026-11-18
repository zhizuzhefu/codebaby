//! 写法建议：validate 的提示与 `fmt --fix` 的自动改写共用同一套规则（规则名可直接 `requestbaby guide <主题>`）。
//!
//! | 规则 | 说明 | 自动改写 |
//! |---|---|---|
//! | `legacy-expect-status` | 旧写法 expectStatus | → expect.status |
//! | `prefer-expect` | 能逐字段表达的 JS 断言 | → expect.status / headers / body |
//! | `prefer-capture` | post 里 `vars.x = data.a.b` 这样的取值 | → capture |
//! | `long-js-test` | 一条断言是一段 JS 程序 | 否（提示拆分） |
//! | `hardcoded-base-url` | URL 写死了环境里的域名 | → {{baseUrl}} |
//! | `redundant-number` | 变量已是数字还包 Number() | → 去掉 Number() |
//! | `env-guard-script` | setup 里用脚本限制环境 | 已有流程固定该环境时删除 |
//!
//! 改写保证语义不变：只改能完整翻译的断言（一条 `a && b` 要么整条改写，要么原样保留）。
use crate::expect::{OPERATORS, is_operator_object};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// 一条写法建议
#[derive(Debug, Clone, PartialEq)]
pub struct Advice {
    pub rule: &'static str,
    pub message: String,
    /// `requestbaby fmt --fix` 能自动改写
    pub fixable: bool,
}

fn advice(rule: &'static str, fixable: bool, message: String) -> Advice {
    Advice { rule, message, fixable }
}

/// 项目上下文：各环境的 baseUrl、固定了环境的流程覆盖的目录
#[derive(Debug, Clone, Default)]
pub struct Context {
    /// (环境名, baseUrl)
    pub base_urls: Vec<(String, String)>,
    /// (流程步骤覆盖的目录（绝对路径）, 固定环境, 流程 key)
    pub pinned: Vec<(PathBuf, String, String)>,
}

impl Context {
    pub fn load(root: &Path) -> Context {
        let mut ctx = Context::default();
        for env in crate::config::list_envs(root) {
            if let Ok(f) = crate::config::load_strict::<crate::config::EnvFile>(&crate::config::env_path(root, &env)) {
                if let Some(b) = f.vars.get("baseUrl").map(|v| v.value.trim_end_matches('/').to_string()).filter(|b| b.contains("://")) {
                    ctx.base_urls.push((env, b));
                }
            }
        }
        if let Some(pf) = crate::runner::discover::project_file(root) {
            if let Ok(p) = crate::config::load_strict::<crate::config::Project>(&pf) {
                for (key, wf) in &p.workflows {
                    let Some(env) = &wf.env else { continue };
                    for step in &wf.steps {
                        let dir = root.join(step.run.trim_end_matches('/'));
                        let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
                        ctx.pinned.push((dir, env.clone(), key.clone()));
                    }
                }
            }
        }
        ctx
    }

    /// 覆盖 dir 的固定环境流程：(环境, 流程 key)
    fn pinned_for(&self, dir: &Path) -> Option<(&str, &str)> {
        let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        self.pinned.iter().find(|(d, _, _)| dir.starts_with(d)).map(|(_, e, k)| (e.as_str(), k.as_str()))
    }
}

/// 用例：返回改写后的文档（只含可自动改写的部分）与全部建议
pub fn upgrade_case(doc: &Value, ctx: &Context) -> (Value, Vec<Advice>) {
    let mut d = doc.clone();
    let mut out = vec![];
    let Some(obj) = d.as_object_mut() else { return (d, out) };
    legacy_status(obj, &mut out);
    prefer_expect(obj, &mut out);
    prefer_capture(obj, &mut out);
    hardcoded_base_url(obj, ctx, &mut out);
    redundant_number(obj, &mut out);
    (d, out)
}

/// 集合（collection.json）：dir 为集合目录
pub fn upgrade_collection(doc: &Value, dir: &Path, ctx: &Context) -> (Value, Vec<Advice>) {
    let mut d = doc.clone();
    let mut out = vec![];
    let Some(obj) = d.as_object_mut() else { return (d, out) };
    let Some(setup) = obj.get("setup").and_then(Value::as_str).map(str::to_string) else { return (d, out) };
    let Some((guarded, rest)) = strip_env_guard(&setup) else { return (d, out) };
    match ctx.pinned_for(dir) {
        Some((env, wf)) => {
            out.push(advice(
                "env-guard-script",
                true,
                format!("setup 用脚本限制环境（{guarded}）：流程 {wf} 已固定环境 {env}，verify 会自动跳过其他环境，这段检查可以删掉（requestbaby fmt --fix · guide env）"),
            ));
            if rest.trim().is_empty() {
                obj.remove("setup");
            } else {
                obj.insert("setup".into(), Value::String(rest));
            }
        }
        None => out.push(advice(
            "env-guard-script",
            false,
            format!("setup 用脚本限制环境（{guarded}）：改为在流程里固定环境（requestbaby.json workflows.<流程>.env），verify 会自动跳过其他环境，不再报错（guide env）"),
        )),
    }
    (d, out)
}

// ---------------------------------------------------------------- 规则

fn legacy_status(obj: &mut Map<String, Value>, out: &mut Vec<Advice>) {
    let Some(old) = obj.get("expectStatus").cloned() else { return };
    let cur = obj.get("expect").and_then(|e| e.get("status")).cloned();
    match cur {
        Some(c) if c != old => return, // 两处不一致：validate 报 expect-status-dup，需人工决定
        _ => {}
    }
    out.push(advice("legacy-expect-status", true, format!("expectStatus 是旧写法：改为 \"expect\": {{\"status\": {old}}}（requestbaby fmt --fix）")));
    obj.remove("expectStatus");
    let expect = expect_obj(obj);
    if !expect.contains_key("status") {
        // status 放在最前，符合规范顺序
        let rest = std::mem::take(expect);
        expect.insert("status".into(), old);
        expect.extend(rest);
    }
}

fn expect_obj(obj: &mut Map<String, Value>) -> &mut Map<String, Value> {
    let e = obj.entry("expect").or_insert_with(|| json!({}));
    if !e.is_object() {
        *e = json!({});
    }
    e.as_object_mut().expect("object")
}

/// JS 断言 → expect：整条可翻译才改写；长段程序给出拆分建议
fn prefer_expect(obj: &mut Map<String, Value>, out: &mut Vec<Advice>) {
    let Some(tests) = obj.get("tests").and_then(Value::as_array).cloned() else { return };
    let mut keep = vec![];
    let mut expect = obj.get("expect").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut converted = vec![];
    for (i, t) in tests.iter().enumerate() {
        let Some(src) = t.as_str() else {
            keep.push(t.clone());
            continue;
        };
        let attempt = translate_test(src).and_then(|items| {
            let mut next = expect.clone();
            items.iter().try_for_each(|a| merge(&mut next, a)).map(|_| (next, items))
        });
        match attempt {
            Some((next, items)) => {
                expect = next;
                converted.push(format!("第 {} 条 → {}", i + 1, items.iter().map(Assertion::snippet).collect::<Vec<_>>().join("，")));
            }
            None => {
                let len = src.chars().count();
                if len > 120 || src.contains("function") || src.contains("return ") {
                    out.push(advice(
                        "long-js-test",
                        false,
                        format!(
                            "tests 第 {} 条是一段 {len} 字符的 JS 程序：能逐字段写的检查（等于 / 类型 / 长度 / 每个元素满足 [*]）改用 expect，只把需要计算的留在 tests，失败时才能看出是哪个字段（guide expect）",
                            i + 1
                        ),
                    ));
                }
                keep.push(t.clone());
            }
        }
    }
    if converted.is_empty() {
        return;
    }
    out.push(advice("prefer-expect", true, format!("JS 断言可以写成声明式 expect：{}（requestbaby fmt --fix · guide expect）", converted.join("；"))));
    obj.insert("expect".into(), Value::Object(expect));
    if keep.is_empty() {
        obj.remove("tests");
    } else {
        obj.insert("tests".into(), Value::Array(keep));
    }
}

/// post 里的简单取值 `vars.x = <响应路径>` → capture；其余语句原样保留
fn prefer_capture(obj: &mut Map<String, Value>, out: &mut Vec<Advice>) {
    let Some(post) = obj.get("post").and_then(Value::as_str).map(str::to_string) else { return };
    let mut capture = obj.get("capture").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut keep = vec![];
    let mut moved = vec![];
    for stmt in split_statements(&post) {
        match translate_capture(&stmt) {
            Some((name, from)) if capture.get(&name).is_none_or(|v| v.as_str() == Some(from.as_str())) => {
                moved.push(format!("\"{name}\": \"{from}\""));
                capture.insert(name, Value::String(from));
            }
            _ => keep.push(stmt),
        }
    }
    if moved.is_empty() {
        return;
    }
    out.push(advice(
        "prefer-capture",
        true,
        format!("post 里的取值可以写成 \"capture\": {{{}}}；取不到时用例直接失败并写明缺哪个字段（requestbaby fmt --fix · guide capture）", moved.join(", ")),
    ));
    obj.insert("capture".into(), Value::Object(capture));
    let rest = keep.join("\n");
    if rest.trim().is_empty() {
        obj.remove("post");
    } else {
        obj.insert("post".into(), Value::String(rest));
    }
}

fn hardcoded_base_url(obj: &mut Map<String, Value>, ctx: &Context, out: &mut Vec<Advice>) {
    let Some(url) = obj.get("request").and_then(|r| r.get("url")).and_then(Value::as_str).map(str::to_string) else { return };
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return;
    }
    let hit = ctx.base_urls.iter().filter(|(_, b)| url == *b || url.starts_with(&format!("{b}/")) || url.starts_with(&format!("{b}?"))).max_by_key(|(_, b)| b.len());
    match hit {
        Some((env, base)) => {
            let fixed = format!("{{{{baseUrl}}}}{}", &url[base.len()..]);
            out.push(advice(
                "hardcoded-base-url",
                true,
                format!("URL 写死了环境 {env} 的地址：改为 {fixed}，换环境（-e）时自动跟着变（requestbaby fmt --fix · guide env）"),
            ));
            if let Some(req) = obj.get_mut("request").and_then(Value::as_object_mut) {
                req.insert("url".into(), Value::String(fixed));
            }
        }
        None => out.push(advice(
            "hardcoded-base-url",
            false,
            "URL 写死了域名：把域名放进环境变量 baseUrl（envs/<环境>.json），URL 写 {{baseUrl}}/路径，用例才能在不同环境运行（guide env）".into(),
        )),
    }
}

/// `Number(env.x)` / `Number(vars.x)`：x 在每一行数据里都已经是数字 → 多余
fn redundant_number(obj: &mut Map<String, Value>, out: &mut Vec<Advice>) {
    let Some(rows) = obj.get("cases").and_then(Value::as_array) else { return };
    let numeric: Vec<String> = {
        let mut keys: Vec<String> = rows.iter().filter_map(Value::as_object).flat_map(|r| r.keys().cloned()).collect();
        keys.sort();
        keys.dedup();
        keys.into_iter()
            .filter(|k| rows.iter().all(|r| r.get(k).is_some_and(Value::is_number)))
            .collect()
    };
    if numeric.is_empty() {
        return;
    }
    let mut hits = vec![];
    let fix = |s: &str, hits: &mut Vec<String>| -> String {
        let mut s = s.to_string();
        for k in &numeric {
            for scope in ["env", "vars"] {
                let pat = format!("Number({scope}.{k})");
                if s.contains(&pat) {
                    hits.push(pat.clone());
                    s = s.replace(&pat, &format!("{scope}.{k}"));
                }
            }
        }
        s
    };
    if let Some(tests) = obj.get_mut("tests").and_then(Value::as_array_mut) {
        for t in tests.iter_mut() {
            if let Some(src) = t.as_str() {
                *t = Value::String(fix(src, &mut hits));
            }
        }
    }
    for key in ["pre", "post"] {
        if let Some(src) = obj.get(key).and_then(Value::as_str).map(str::to_string) {
            obj.insert(key.into(), Value::String(fix(&src, &mut hits)));
        }
    }
    if !hits.is_empty() {
        hits.dedup();
        out.push(advice(
            "redundant-number",
            true,
            format!("{} 多余：数据行里已经是数字，脚本里直接用（requestbaby fmt --fix · guide data）", hits.join("、")),
        ));
    }
}

/// `if (env.x !== '…') throw new Error(…)` 开头的 setup：返回 (守卫条件, 其余脚本)
fn strip_env_guard(src: &str) -> Option<(String, String)> {
    let re = regex::Regex::new(
        r#"^\s*if\s*\(\s*(env\.[A-Za-z_$][\w$]*\s*!==?\s*(?:'[^']*'|"[^"]*")|(?:'[^']*'|"[^"]*")\s*!==?\s*env\.[A-Za-z_$][\w$]*)\s*\)\s*\{?\s*throw\s+new\s+Error\((?:'[^']*'|"[^"]*"|`[^`]*`)\)\s*;?\s*\}?\s*;?"#,
    )
    .expect("regex");
    let m = re.captures(src)?;
    let cond = m.get(1)?.as_str().split_whitespace().collect::<Vec<_>>().join(" ");
    Some((cond, src[m.get(0)?.end()..].trim().to_string()))
}

// ---------------------------------------------------------------- JS 断言 → expect

/// 翻译出的一条声明式断言
#[derive(Debug, Clone, PartialEq)]
enum Assertion {
    Status(Vec<u16>),
    /// (路径, 运算符或 "=", 值)
    Body(String, String, Value),
    Header(String, String, Value),
}

impl Assertion {
    fn matcher(op: &str, v: &Value) -> Value {
        if op == "=" { v.clone() } else { Value::Object(Map::from_iter([(op.to_string(), v.clone())])) }
    }

    fn snippet(&self) -> String {
        match self {
            Assertion::Status(s) if s.len() == 1 => format!("status {}", s[0]),
            Assertion::Status(s) => format!("status {s:?}"),
            Assertion::Body(p, op, v) => format!("\"{p}\": {}", crate::expect::inline_json(&Self::matcher(op, v))),
            Assertion::Header(h, op, v) => format!("header \"{h}\": {}", crate::expect::inline_json(&Self::matcher(op, v))),
        }
    }
}

/// 把一条断言并进 expect：同一路径的多个条件合并成一个运算符对象；冲突则失败（整条保留为 JS）
fn merge(expect: &mut Map<String, Value>, a: &Assertion) -> Option<()> {
    let (section, key, op, v) = match a {
        Assertion::Status(list) => {
            let v = if list.len() == 1 { json!(list[0]) } else { json!(list) };
            return match expect.get("status") {
                Some(cur) if *cur != v => None,
                _ => {
                    expect.insert("status".into(), v);
                    Some(())
                }
            };
        }
        Assertion::Body(p, op, v) => ("body", p, op, v),
        Assertion::Header(h, op, v) => ("headers", h, op, v),
    };
    let sec = expect.entry(section).or_insert_with(|| json!({})).as_object_mut()?;
    let new = Assertion::matcher(op, v);
    match sec.get(key.as_str()).cloned() {
        None => {
            sec.insert(key.clone(), new);
        }
        Some(cur) if cur == new => {}
        Some(cur) => {
            // 合并成运算符对象：字面量 L → {"eq": L}
            let as_ops = |x: &Value| -> Map<String, Value> {
                if is_operator_object(x) { x.as_object().cloned().unwrap_or_default() } else { Map::from_iter([("eq".to_string(), x.clone())]) }
            };
            let mut ops = as_ops(&cur);
            for (k, x) in as_ops(&new) {
                match ops.get(&k) {
                    Some(y) if *y != x => return None,
                    _ => {
                        ops.insert(k, x);
                    }
                }
            }
            sec.insert(key.clone(), Value::Object(ops));
        }
    }
    Some(())
}

/// 一条 JS 断言 → 声明式断言（`a && b && c` 每段都能翻译才算成功）
fn translate_test(src: &str) -> Option<Vec<Assertion>> {
    let expr = Parser::parse(src)?;
    let mut clauses = vec![];
    flatten_and(expr, &mut clauses);
    let items: Vec<Assertion> = clauses.into_iter().map(|c| translate_clause(&c)).collect::<Option<_>>()?;
    // JS 里 a.b.c === undefined 在 a.b 不存在（或为 null）时会抛错 = 失败；expect 的 exists:false 会通过。
    // 补上「父级存在且不为 null」，结论与原 JS 一致
    let mut out = vec![];
    for a in items {
        if let Assertion::Body(path, op, Value::Bool(false)) = &a {
            if op == "exists" {
                if let Some(parent) = parent_path(path) {
                    out.push(Assertion::Body(parent.clone(), "exists".into(), json!(true)));
                    out.push(Assertion::Body(parent, "ne".into(), Value::Null));
                }
            }
        }
        out.push(a);
    }
    Some(out)
}

/// data.a.b → data.a；items[0] → items；顶层字段 → None
fn parent_path(path: &str) -> Option<String> {
    let cut = path.rfind(['.', '['])?;
    Some(path[..cut].to_string()).filter(|p| !p.is_empty())
}

fn flatten_and(e: Expr, out: &mut Vec<Expr>) {
    match e {
        Expr::Bin(op, l, r) if op == "&&" => {
            flatten_and(*l, out);
            flatten_and(*r, out);
        }
        other => out.push(other),
    }
}

fn translate_clause(e: &Expr) -> Option<Assertion> {
    match e {
        Expr::Bin(op, l, r) if is_cmp(op) => {
            // 统一成「目标 op 值」：值在左边时翻转比较方向
            if let Some(a) = cmp_assertion(op, l, r) {
                return Some(a);
            }
            cmp_assertion(&flip(op), r, l)
        }
        // [200, 201].includes(res.status)
        Expr::Call(callee, args) => {
            let (base, method) = member(callee)?;
            match (method, args.as_slice()) {
                ("includes", [arg]) if is_status(arg) => match base {
                    Expr::Array(items) => Some(Assertion::Status(items.iter().map(status_code).collect::<Option<Vec<_>>>()?)),
                    _ => None,
                },
                ("includes" | "startsWith" | "endsWith", [arg]) => {
                    let op = if method == "includes" { "contains" } else { method };
                    target(base, op, value(arg)?)
                }
                ("test", [arg]) => match base {
                    Expr::Regex(re, flags) if flags.is_empty() => target(arg, "matches", Value::String(re.clone())),
                    _ => None,
                },
                ("isArray", [arg]) if matches!(base, Expr::Ident(n) if n == "Array") => target(arg, "type", json!("array")),
                ("isInteger", [arg]) if matches!(base, Expr::Ident(n) if n == "Number") => target(arg, "type", json!("integer")),
                _ => None,
            }
        }
        _ => None,
    }
}

fn is_cmp(op: &str) -> bool {
    matches!(op, "===" | "==" | "!==" | "!=" | ">" | ">=" | "<" | "<=")
}

fn flip(op: &str) -> String {
    match op {
        ">" => "<",
        ">=" => "<=",
        "<" => ">",
        "<=" => ">=",
        o => o,
    }
    .to_string()
}

/// `目标 op 值`
fn cmp_assertion(op: &str, l: &Expr, r: &Expr) -> Option<Assertion> {
    // res.status === 200
    if is_status(l) {
        return match op {
            "===" | "==" => Some(Assertion::Status(vec![status_code(r)?])),
            _ => None,
        };
    }
    // typeof x === 'string'
    if let Expr::Typeof(inner) = l {
        let Expr::Str(t) = r else { return None };
        return match (op, t.as_str()) {
            ("===" | "==", "undefined") => target(inner, "exists", json!(false)),
            ("!==" | "!=", "undefined") => target(inner, "exists", json!(true)),
            // JS 的 typeof 没有 integer / null / array，'object' 还包括数组与 null：只翻译含义一致的
            ("===" | "==", t @ ("string" | "number" | "boolean")) => target(inner, "type", json!(t)),
            _ => None,
        };
    }
    // x.length op n
    if let Some((base, "length")) = member(l) {
        if path_of(base).is_some() {
            let v = value(r)?;
            let m = match op {
                "===" | "==" => v,
                ">" | ">=" | "<" | "<=" | "!==" | "!=" => json!({ op_name(op)?: v }),
                _ => return None,
            };
            return target(base, "len", m);
        }
    }
    // x === undefined / x !== undefined
    if matches!(r, Expr::Ident(n) if n == "undefined") {
        return match op {
            "===" | "==" => target(l, "exists", json!(false)),
            "!==" | "!=" => target(l, "exists", json!(true)),
            _ => None,
        };
    }
    let v = value(r)?;
    // == 的隐式转换（'0' == 0、null == undefined）无法逐字表达：只翻译不受影响的情况
    if matches!(op, "==" | "!=") && (v.is_null() || v.is_string()) {
        return None;
    }
    if matches!(op, ">" | ">=" | "<" | "<=") && !(v.is_number() || is_placeholder(&v)) {
        return None;
    }
    match op {
        "===" | "==" => target(l, "=", v),
        o => target(l, op_name(o)?, v),
    }
}

fn op_name(op: &str) -> Option<&'static str> {
    Some(match op {
        "!==" | "!=" => "ne",
        ">" => "gt",
        ">=" => "gte",
        "<" => "lt",
        "<=" => "lte",
        _ => return None,
    })
}

fn is_placeholder(v: &Value) -> bool {
    v.as_str().is_some_and(|s| s.starts_with("{{") && s.ends_with("}}") && s.matches("{{").count() == 1)
}

/// 断言对象：响应体路径或响应头
fn target(e: &Expr, op: &str, v: Value) -> Option<Assertion> {
    debug_assert!(op == "=" || OPERATORS.contains(&op));
    if let Some(h) = header_of(e) {
        return Some(Assertion::Header(h, op.into(), v));
    }
    Some(Assertion::Body(path_of(e)?, op.into(), v))
}

fn is_status(e: &Expr) -> bool {
    matches!(member(e), Some((Expr::Ident(r), "status")) if r == "res")
}

fn status_code(e: &Expr) -> Option<u16> {
    match e {
        Expr::Num(n) => n.parse().ok(),
        _ => None,
    }
}

fn member(e: &Expr) -> Option<(&Expr, &str)> {
    match e {
        Expr::Member(b, k) => Some((b, k.as_str())),
        _ => None,
    }
}

/// 响应体路径：data.a.b / data['a'][0] / res.json().a / res.json('a.b.0')
fn path_of(e: &Expr) -> Option<String> {
    let key_ok = |k: &str| !k.is_empty() && k != "*" && !k.contains(['.', '[', ']']) && !k.starts_with('$');
    match e {
        Expr::Ident(n) if n == "data" => Some(String::new()),
        Expr::Call(callee, args) if matches!(member(callee), Some((Expr::Ident(r), "json")) if r == "res") => match args.as_slice() {
            [] => Some(String::new()),
            [Expr::Str(p)] if p.split('.').all(key_ok) => Some(p.clone()),
            _ => None,
        },
        Expr::Member(base, k) => {
            let b = path_of(base)?;
            if !key_ok(k) || k == "length" {
                return None;
            }
            Some(if b.is_empty() { k.clone() } else { format!("{b}.{k}") })
        }
        Expr::Index(base, idx) => {
            let b = path_of(base)?;
            match &**idx {
                Expr::Num(n) if n.chars().all(|c| c.is_ascii_digit()) => Some(format!("{b}[{n}]")),
                Expr::Str(k) if key_ok(k) => Some(if b.is_empty() { k.clone() } else { format!("{b}.{k}") }),
                _ => None,
            }
        }
        _ => None,
    }
}

/// 响应头：res.headers['content-type'] / res.headers.etag
fn header_of(e: &Expr) -> Option<String> {
    let is_headers = |b: &Expr| matches!(member(b), Some((Expr::Ident(r), "headers")) if r == "res");
    match e {
        Expr::Index(b, k) if is_headers(b) => match &**k {
            Expr::Str(h) => Some(h.clone()),
            _ => None,
        },
        Expr::Member(b, k) if is_headers(b) => Some(k.clone()),
        _ => None,
    }
}

/// 期望值：字面量、env.x / vars.x（→ {{x}}）、Number(env.x)、'前缀' + env.x 拼接
fn value(e: &Expr) -> Option<Value> {
    match e {
        Expr::Num(n) => serde_json::from_str(n).ok(),
        Expr::Str(s) => Some(Value::String(s.clone())),
        Expr::Ident(n) if n == "true" => Some(json!(true)),
        Expr::Ident(n) if n == "false" => Some(json!(false)),
        Expr::Ident(n) if n == "null" => Some(Value::Null),
        Expr::Member(b, k) if matches!(&**b, Expr::Ident(s) if s == "env" || s == "vars") => Some(Value::String(format!("{{{{{k}}}}}"))),
        Expr::Index(b, k) if matches!(&**b, Expr::Ident(s) if s == "env" || s == "vars") => match &**k {
            Expr::Str(k) => Some(Value::String(format!("{{{{{k}}}}}"))),
            _ => None,
        },
        // Number(env.x) / String(env.x)：expect 里 "{{x}}" 与数字按文本比较，不需要转换
        Expr::Call(f, args) if matches!(&**f, Expr::Ident(n) if n == "Number" || n == "String") => match args.as_slice() {
            [a] => value(a).filter(is_placeholder),
            _ => None,
        },
        Expr::Bin(op, l, r) if op == "+" => {
            let (l, r) = (value(l)?, value(r)?);
            match (l, r) {
                (Value::String(a), Value::String(b)) => Some(Value::String(a + &b)),
                _ => None,
            }
        }
        Expr::Array(items) => items.iter().map(value).collect::<Option<Vec<_>>>().map(Value::Array),
        _ => None,
    }
}

/// `vars.x = <响应路径>` / `vars.x = res.headers['h']`
fn translate_capture(stmt: &str) -> Option<(String, String)> {
    let e = Parser::parse(stmt)?;
    let Expr::Assign(lhs, rhs) = e else { return None };
    let name = match &*lhs {
        Expr::Member(b, k) if matches!(&**b, Expr::Ident(v) if v == "vars") => k.clone(),
        _ => return None,
    };
    if let Some(h) = header_of(&rhs) {
        return Some((name, format!("header:{h}")));
    }
    let p = path_of(&rhs)?;
    (!p.is_empty()).then_some((name, p))
}

/// 按顶层的 ; 与换行切分语句（引号 / 括号内不切）
fn split_statements(src: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in src.chars() {
        if let Some(q) = quote {
            cur.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '\'' | '"' | '`' => {
                quote = Some(c);
                cur.push(c);
            }
            '(' | '[' | '{' => {
                depth += 1;
                cur.push(c);
            }
            ')' | ']' | '}' => {
                depth -= 1;
                cur.push(c);
            }
            ';' | '\n' if depth == 0 => {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_string());
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

// ---------------------------------------------------------------- 迷你 JS 表达式解析（只覆盖断言常见写法）

#[derive(Debug, Clone, PartialEq)]
enum Expr {
    Ident(String),
    /// 数字原文
    Num(String),
    Str(String),
    Regex(String, String),
    Array(Vec<Expr>),
    Member(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    Call(Box<Expr>, Vec<Expr>),
    Bin(String, Box<Expr>, Box<Expr>),
    Typeof(Box<Expr>),
    Assign(Box<Expr>, Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Num(String),
    Str(String),
    Regex(String, String),
    Punct(&'static str),
}

fn lex(src: &str) -> Option<Vec<Tok>> {
    const PUNCT: &[&str] = &["===", "!==", "==", "!=", ">=", "<=", "&&", "||", ">", "<", "(", ")", "[", "]", ".", ",", "+", "-", "=", "!"];
    let chars: Vec<char> = src.chars().collect();
    let mut toks = vec![];
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_digit() {
            let start = i;
            // 小数点后必须是数字：data.items.0.id 里的 0 后面是成员访问
            while i < chars.len() && (chars[i].is_ascii_digit() || (chars[i] == '.' && chars.get(i + 1).is_some_and(|c| c.is_ascii_digit()))) {
                i += 1;
            }
            toks.push(Tok::Num(chars[start..i].iter().collect()));
            continue;
        }
        if c.is_alphabetic() || c == '_' || c == '$' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$') {
                i += 1;
            }
            toks.push(Tok::Ident(chars[start..i].iter().collect()));
            continue;
        }
        if c == '\'' || c == '"' {
            let mut s = String::new();
            i += 1;
            loop {
                let ch = *chars.get(i)?;
                i += 1;
                match ch {
                    '\\' => {
                        let e = *chars.get(i)?;
                        i += 1;
                        s.push(match e {
                            'n' => '\n',
                            't' => '\t',
                            other => other,
                        });
                    }
                    q if q == c => break,
                    other => s.push(other),
                }
            }
            toks.push(Tok::Str(s));
            continue;
        }
        // 正则字面量：出现在「值」的位置（开头 / 运算符或左括号之后）
        if c == '/' && matches!(toks.last(), None | Some(Tok::Punct(_))) && !matches!(toks.last(), Some(Tok::Punct(")" | "]"))) {
            let mut re = String::new();
            i += 1;
            let mut class = false;
            loop {
                let ch = *chars.get(i)?;
                i += 1;
                match ch {
                    '\\' => {
                        let e = *chars.get(i)?;
                        i += 1;
                        if e != '/' {
                            re.push('\\');
                        }
                        re.push(e);
                    }
                    '[' => {
                        class = true;
                        re.push(ch);
                    }
                    ']' => {
                        class = false;
                        re.push(ch);
                    }
                    '/' if !class => break,
                    other => re.push(other),
                }
            }
            let start = i;
            while i < chars.len() && chars[i].is_ascii_alphabetic() {
                i += 1;
            }
            toks.push(Tok::Regex(re, chars[start..i].iter().collect()));
            continue;
        }
        let rest: String = chars[i..chars.len().min(i + 3)].iter().collect();
        let p = PUNCT.iter().find(|p| rest.starts_with(**p))?;
        toks.push(Tok::Punct(p));
        i += p.chars().count();
    }
    Some(toks)
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn parse(src: &str) -> Option<Expr> {
        let mut p = Parser { toks: lex(src.trim().trim_end_matches(';'))?, pos: 0 };
        let e = p.assign()?;
        (p.pos == p.toks.len()).then_some(e)
    }

    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn eat(&mut self, p: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Punct(x)) if *x == p) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn assign(&mut self) -> Option<Expr> {
        let l = self.and()?;
        if self.eat("=") {
            let r = self.and()?;
            return Some(Expr::Assign(Box::new(l), Box::new(r)));
        }
        Some(l)
    }

    fn and(&mut self) -> Option<Expr> {
        let mut l = self.cmp()?;
        while self.eat("&&") {
            let r = self.cmp()?;
            l = Expr::Bin("&&".into(), Box::new(l), Box::new(r));
        }
        if matches!(self.peek(), Some(Tok::Punct("||"))) {
            return None;
        }
        Some(l)
    }

    fn cmp(&mut self) -> Option<Expr> {
        let l = self.add()?;
        for op in ["===", "!==", "==", "!=", ">=", "<=", ">", "<"] {
            if self.eat(op) {
                let r = self.add()?;
                return Some(Expr::Bin(op.into(), Box::new(l), Box::new(r)));
            }
        }
        Some(l)
    }

    fn add(&mut self) -> Option<Expr> {
        let mut l = self.unary()?;
        while self.eat("+") {
            let r = self.unary()?;
            l = Expr::Bin("+".into(), Box::new(l), Box::new(r));
        }
        Some(l)
    }

    fn unary(&mut self) -> Option<Expr> {
        if matches!(self.peek(), Some(Tok::Ident(t)) if t == "typeof") {
            self.pos += 1;
            return Some(Expr::Typeof(Box::new(self.postfix()?)));
        }
        if self.eat("-") {
            return match self.postfix()? {
                Expr::Num(n) => Some(Expr::Num(format!("-{n}"))),
                _ => None,
            };
        }
        if matches!(self.peek(), Some(Tok::Punct("!"))) {
            return None;
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Option<Expr> {
        let mut e = self.primary()?;
        loop {
            if self.eat(".") {
                match self.toks.get(self.pos).cloned() {
                    Some(Tok::Ident(k)) => {
                        self.pos += 1;
                        e = Expr::Member(Box::new(e), k);
                    }
                    _ => return None,
                }
            } else if self.eat("[") {
                let idx = self.and()?;
                if !self.eat("]") {
                    return None;
                }
                e = Expr::Index(Box::new(e), Box::new(idx));
            } else if self.eat("(") {
                let mut args = vec![];
                if !self.eat(")") {
                    loop {
                        args.push(self.and()?);
                        if self.eat(")") {
                            break;
                        }
                        if !self.eat(",") {
                            return None;
                        }
                    }
                }
                e = Expr::Call(Box::new(e), args);
            } else {
                return Some(e);
            }
        }
    }

    fn primary(&mut self) -> Option<Expr> {
        let t = self.toks.get(self.pos).cloned()?;
        self.pos += 1;
        match t {
            Tok::Ident(n) if matches!(n.as_str(), "function" | "new" | "return" | "const" | "let" | "var") => None,
            Tok::Ident(n) => Some(Expr::Ident(n)),
            Tok::Num(n) => Some(Expr::Num(n)),
            Tok::Str(s) => Some(Expr::Str(s)),
            Tok::Regex(r, f) => Some(Expr::Regex(r, f)),
            Tok::Punct("(") => {
                let e = self.and()?;
                self.eat(")").then_some(e)
            }
            Tok::Punct("[") => {
                let mut items = vec![];
                if self.eat("]") {
                    return Some(Expr::Array(items));
                }
                loop {
                    items.push(self.and()?);
                    if self.eat("]") {
                        return Some(Expr::Array(items));
                    }
                    if !self.eat(",") {
                        return None;
                    }
                }
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upgrade(doc: Value) -> (Value, Vec<Advice>) {
        upgrade_case(&doc, &Context { base_urls: vec![("dev".into(), "http://127.0.0.1:8080".into())], pinned: vec![] })
    }

    fn rules(a: &[Advice]) -> Vec<&'static str> {
        a.iter().map(|x| x.rule).collect()
    }

    #[test]
    fn translates_common_js_assertions() {
        let t = |s: &str| translate_test(s).map(|v| v.iter().map(Assertion::snippet).collect::<Vec<_>>().join(" | "));
        assert_eq!(t("res.status === 200").unwrap(), "status 200");
        assert_eq!(t("[200, 201].includes(res.status)").unwrap(), "status [200, 201]");
        assert_eq!(t("data.code === 0 && data.data.hasTeacherNote === true").unwrap(), r#""code": 0 | "data.hasTeacherNote": true"#);
        assert_eq!(t("res.json().data.items.length === 2").unwrap(), r#""data.items": {"len": 2}"#);
        assert_eq!(t("data.list.length > 0").unwrap(), r#""list": {"len": {"gt": 0}}"#);
        assert_eq!(t("res.json('data.files.0.pageId') === 'p1'").unwrap(), r#""data.files.0.pageId": "p1""#);
        assert_eq!(t("data['items'][0].id !== undefined").unwrap(), r#""items[0].id": {"exists": true}"#);
        assert_eq!(t("data.a.gone === undefined").unwrap(), r#""a": {"exists": true} | "a": {"ne": null} | "a.gone": {"exists": false}"#);
        assert_eq!(t("data.gone === undefined").unwrap(), r#""gone": {"exists": false}"#);
        assert_eq!(t("typeof data.token === 'string'").unwrap(), r#""token": {"type": "string"}"#);
        assert_eq!(t("Array.isArray(data.list)").unwrap(), r#""list": {"type": "array"}"#);
        assert_eq!(t("Number.isInteger(data.id)").unwrap(), r#""id": {"type": "integer"}"#);
        assert_eq!(t("data.url.startsWith(env.baseUrl + '/img?')").unwrap(), r#""url": {"startsWith": "{{baseUrl}}/img?"}"#);
        assert_eq!(t("data.msg.includes('ok')").unwrap(), r#""msg": {"contains": "ok"}"#);
        assert_eq!(t("/^\\d+$/.test(data.id)").unwrap(), r#""id": {"matches": "^\\d+$"}"#);
        assert_eq!(t("data.total >= 1 && data.total < 100").unwrap(), r#""total": {"gte": 1} | "total": {"lt": 100}"#);
        assert_eq!(t("0 < data.total").unwrap(), r#""total": {"gt": 0}"#);
        assert_eq!(t("data.count === Number(env.stemCount)").unwrap(), r#""count": "{{stemCount}}""#);
        assert_eq!(t("data.uid === vars.uid").unwrap(), r#""uid": "{{uid}}""#);
        assert_eq!(t("res.headers['content-type'].includes('json')").unwrap(), r#"header "content-type": {"contains": "json"}"#);
        assert_eq!(t("data.price === -1.5").unwrap(), r#""price": -1.5"#);
        // 不能逐字表达的保持 JS
        for keep in [
            "data.list.every(x => x.price > 0)",
            "data.a === 1 || data.b === 1",
            "data.code == '0'",
            "data.code == null",
            "!data.error",
            "data.list.map(x => x.id).join(',') === env.ids",
            "(()=>{ return true })()",
            "data.a.length",
            "res.status >= 200",
            "data.name > 'a'",
            "/x/i.test(data.a)",
            "typeof data.list === 'object'",
            "data.items.0.id === 1",
        ] {
            assert!(t(keep).is_none(), "{keep}");
        }
    }

    #[test]
    fn upgrade_rewrites_tests_post_status_and_url() {
        let (doc, adv) = upgrade(json!({
            "expectStatus": 200,
            "request": {"method": "GET", "url": "http://127.0.0.1:8080/api/x?a=1"},
            "post": "vars.token = res.json('data.token');\nvars.trace = res.headers['x-trace-id']\nconst x = data.list.map(i => i.id); vars.ids = x.join(',')",
            "tests": ["data.code === 0", "data.total > 0 && data.total <= 10", "data.list.every(x => x.ok)", "res.status === 200"],
        }));
        assert_eq!(rules(&adv), ["legacy-expect-status", "prefer-expect", "prefer-capture", "hardcoded-base-url"]);
        assert!(adv.iter().all(|a| a.fixable));
        assert_eq!(doc["expect"], json!({"status": 200, "body": {"code": 0, "total": {"gt": 0, "lte": 10}}}));
        assert_eq!(doc["tests"], json!(["data.list.every(x => x.ok)"]));
        assert_eq!(doc["capture"], json!({"token": "data.token", "trace": "header:x-trace-id"}));
        assert_eq!(doc["post"], "const x = data.list.map(i => i.id)\nvars.ids = x.join(',')");
        assert_eq!(doc["request"]["url"], "{{baseUrl}}/api/x?a=1");
        assert!(doc.get("expectStatus").is_none());
        // 改写结果是合法用例，且已是最终形态（再跑一遍没有可改写项）
        serde_json::from_value::<crate::config::TestCase>(doc.clone()).unwrap();
        let (again, adv2) = upgrade(doc.clone());
        assert_eq!(again, doc);
        assert!(adv2.iter().all(|a| !a.fixable));
    }

    #[test]
    fn conflicts_and_long_programs_are_left_alone() {
        // 与已有 expect 冲突的断言保留为 JS；状态码不同不合并
        let (doc, adv) = upgrade(json!({
            "request": {"method": "GET", "url": "{{baseUrl}}/x"},
            "expect": {"status": 201, "body": {"code": 0}},
            "tests": ["data.code === 1", "res.status === 200", "(()=>{const pages=data.files.reduce((a,f)=>a.concat(f.pages),[]); return pages.length === 3;})()"],
        }));
        assert_eq!(doc["tests"].as_array().unwrap().len(), 3);
        assert_eq!(rules(&adv), ["long-js-test"]);
        assert!(!adv[0].fixable && adv[0].message.contains("第 3 条"));
        // 写死的域名不属于任何环境：只提示
        let (_, adv) = upgrade(json!({"request": {"method": "GET", "url": "https://other.example.com/x"}}));
        assert_eq!(rules(&adv), ["hardcoded-base-url"]);
        assert!(!adv[0].fixable);
    }

    #[test]
    fn redundant_number_only_when_rows_are_numbers() {
        let (doc, adv) = upgrade(json!({
            "request": {"method": "GET", "url": "{{baseUrl}}/x"},
            "tests": ["data.list.filter(x => x.ok).length === Number(env.n)"],
            "cases": [{"n": 2}, {"n": 3}],
        }));
        assert!(rules(&adv).contains(&"redundant-number"));
        assert_eq!(doc["tests"][0], "data.list.filter(x => x.ok).length === env.n");
        let (doc, adv) = upgrade(json!({
            "request": {"method": "GET", "url": "{{baseUrl}}/x"},
            "tests": ["data.list.filter(x => x.ok).length === Number(env.n)"],
            "cases": [{"n": "2"}],
        }));
        assert!(!rules(&adv).contains(&"redundant-number"));
        assert_eq!(doc["tests"][0], "data.list.filter(x => x.ok).length === Number(env.n)");
    }

    #[test]
    fn env_guard_in_setup() {
        let dir = tempfile::tempdir().unwrap();
        let col = dir.path().join("fixtures/a");
        std::fs::create_dir_all(&col).unwrap();
        let doc = json!({"name": "a", "setup": "if(env.baseUrl!=='http://127.0.0.1:18087') throw new Error('本地 fixture 只能使用 local 环境'); "});
        let (_, adv) = upgrade_collection(&doc, &col, &Context::default());
        assert_eq!(rules(&adv), ["env-guard-script"]);
        assert!(!adv[0].fixable && adv[0].message.contains("workflows.<流程>.env"));
        let ctx = Context { base_urls: vec![], pinned: vec![(std::fs::canonicalize(dir.path().join("fixtures")).unwrap(), "local".into(), "fx".into())] };
        let (fixed, adv) = upgrade_collection(&doc, &col, &ctx);
        assert!(adv[0].fixable && adv[0].message.contains("流程 fx 已固定环境 local"));
        assert!(fixed.get("setup").is_none());
        let doc = json!({"setup": "if (env.name !== \"local\") { throw new Error(\"no\") }\nvars.seed = 1"});
        let (fixed, _) = upgrade_collection(&doc, &col, &ctx);
        assert_eq!(fixed["setup"], "vars.seed = 1");
        let (_, adv) = upgrade_collection(&json!({"setup": "if (env.x === 'a') throw new Error('x')"}), &col, &ctx);
        assert!(adv.is_empty(), "不是「不等于就报错」的守卫不动");
    }

    /// 等价性：可翻译的 JS 断言改写成 expect 后，对同一批响应（字段类型正确，值对 / 错 / 缺失 / 空数组）结论与原 JS 完全一致
    #[test]
    fn rewritten_assertions_agree_with_the_original_js() {
        use crate::script::{QuickJsHost, RespData, ScriptHost};
        use crate::vars::{VarVal, Vars};
        use std::collections::BTreeMap;
        let exprs = [
            "res.status === 200",
            "[200, 201].includes(res.status)",
            "data.code === 0",
            "data.code !== 0",
            "data.code === 0 && data.data.ok === true",
            "data.data.total > 0",
            "data.data.total >= 1 && data.data.total < 100",
            "0 < data.data.total",
            "data.data.total === Number(env.n)",
            "data.data.items.length === 2",
            "data.data.items.length > 0",
            "data.data.items[0].sku === env.sku",
            "data['data']['items'][0].sku !== 'B2'",
            "data.data.items[0].qty === 1",
            "res.json('data.items.1.qty') === 3",
            "typeof data.data.token === 'string'",
            "typeof data.data.missing === 'undefined'",
            "data.data.token !== undefined",
            "data.data.gone === undefined",
            "Array.isArray(data.data.items)",
            "Number.isInteger(data.data.total)",
            "data.data.msg.includes('ok')",
            "data.data.url.startsWith(env.base + '/x')",
            "data.data.url.endsWith('e=1')",
            "/^\\d+$/.test(data.data.code_s)",
            "data.data.uid === vars.uid",
            "data.data.flag === null",
            "res.headers['content-type'].includes('json')",
        ];
        let bodies = [
            r#"{"code":0,"data":{"ok":true,"total":2,"items":[{"sku":"A1","qty":1},{"sku":"B2","qty":3}],"token":"tk","msg":"all ok","url":"http://h/x?e=1","code_s":"123","uid":7,"flag":null}}"#,
            r#"{"code":1,"data":{"ok":false,"total":0,"items":[],"token":5,"msg":"bad","url":"ftp://h/x?e=2","code_s":"12a","uid":8,"flag":false,"gone":1}}"#,
            r#"{"code":0,"data":{"ok":true,"total":2.5,"items":[{"sku":"B2","qty":2}],"msg":"ok","url":"http://h/x","code_s":"","uid":7}}"#,
            r#"{"code":0,"data":{"total":150,"items":[{"sku":"A1","qty":1},{"sku":"A1","qty":3},{"sku":"C","qty":3}]}}"#,
            r#"{"code":0,"data":{}}"#,
            r#"{"code":0}"#,
        ];
        let env: BTreeMap<String, VarVal> = [("n", "2"), ("sku", "A1"), ("base", "http://h")].iter().map(|(k, v)| (k.to_string(), VarVal::Str(v.to_string()))).collect();
        let captured: BTreeMap<String, VarVal> = [("uid".to_string(), VarVal::Num(7.0))].into_iter().collect();
        let mut vars = Vars::new();
        vars.push_layer(env.clone());
        vars.push_layer(captured.clone());
        let host = QuickJsHost::new();
        let mut compared = 0;
        for expr in exprs {
            let items = translate_test(expr).unwrap_or_else(|| panic!("应可翻译：{expr}"));
            let mut expect = Map::new();
            for a in &items {
                merge(&mut expect, a).unwrap();
            }
            let e: crate::expect::Expect = serde_json::from_value(Value::Object(expect)).unwrap();
            for body in bodies {
                for (status, ctype) in [(200, "application/json"), (201, "text/plain")] {
                    let headers = vec![("Content-Type".to_string(), ctype.to_string())];
                    let resp = RespData::http(status, 1, body.len() as u64, headers.clone(), body.into());
                    let js = host.eval_tests(&resp, &captured, &env, &[expr.to_string()]).unwrap()[0].pass;
                    let ours = crate::expect::evaluate(&e, &crate::expect::Actual { status, headers: &headers, body, protocol: crate::config::Protocol::Http }, &vars).iter().all(|o| o.pass);
                    assert_eq!(ours, js, "{expr}\n响应 {status} {ctype} {body}");
                    compared += 1;
                }
            }
        }
        assert_eq!(compared, exprs.len() * bodies.len() * 2);
    }
}
