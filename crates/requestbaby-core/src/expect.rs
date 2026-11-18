//! 声明式断言 `expect` 与捕获 `capture`：不写 JS 就能表达绝大多数校验，失败时直接说清「哪个字段、期望什么、实际是什么」。
//!
//! ```json
//! "expect": {
//!   "status": 200,
//!   "headers": { "Content-Type": { "contains": "json" } },
//!   "body": {
//!     "code": 0,
//!     "data.items[*].price": { "gt": 0 },
//!     "data.items": { "len": 2 },
//!     "data.url": { "startsWith": "{{baseUrl}}/" }
//!   }
//! },
//! "capture": { "token": "data.token", "traceId": "header:X-Trace-Id" }
//! ```
//!
//! - 路径：点号分段，`[n]` / `.n` 取数组下标，`[*]` / `*` 表示「每一个元素都要满足」，`$` 表示整个响应体
//! - 期望值：字面量 = 相等（数字按数值比较）；对象且所有键都是运算符 = 运算符检查；其余对象 / 数组 = 整体相等
//! - 期望值中的 `{{var}}`：整个值恰好是一个占位符时保留变量原类型（数据行里的数字仍是数字），否则按字符串渲染
use crate::config::case::ExpectStatus;
use crate::config::Protocol;
use crate::script::TestOutcome;
use crate::vars::{VarVal, Vars};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// 声明式断言
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    /// 状态码：200 或 [200, 201]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ExpectStatus>,
    /// 响应头（名称不区分大小写）→ 期望
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub headers: Map<String, Value>,
    /// 响应体路径 → 期望
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub body: Map<String, Value>,
}

impl Expect {
    pub fn is_empty(&self) -> bool {
        self.status.is_none() && self.headers.is_empty() && self.body.is_empty()
    }
}

/// 运算符（期望值是对象且所有键都在此列表中时按运算符解释）
pub const OPERATORS: &[&str] =
    &["eq", "ne", "gt", "gte", "lt", "lte", "type", "exists", "contains", "startsWith", "endsWith", "matches", "len", "oneOf", "values"];

/// `type` 运算符可选值
pub const TYPES: &[&str] = &["string", "number", "integer", "boolean", "null", "array", "object"];

/// 期望值是否为运算符对象
pub fn is_operator_object(v: &Value) -> bool {
    matches!(v, Value::Object(m) if !m.is_empty() && m.keys().all(|k| OPERATORS.contains(&k.as_str())))
}

/// 响应（断言 / 捕获的输入）
pub struct Actual<'a> {
    pub status: u16,
    pub headers: &'a [(String, String)],
    pub body: &'a str,
    /// 决定 status 怎么读：gRPC 状态码按名字展示、可写 "NOT_FOUND"
    pub protocol: Protocol,
}

/// 求值全部声明式断言（状态码 → 响应头 → 响应体，按书写顺序）
pub fn evaluate(expect: &Expect, actual: &Actual<'_>, vars: &Vars) -> Vec<TestOutcome> {
    let mut out = vec![];
    if let Some(st) = &expect.status {
        let show = |c: &u16| if actual.protocol == Protocol::Grpc { crate::config::protocols::grpc_code_name(*c) } else { c.to_string() };
        match st.resolve_for(actual.protocol, vars) {
            Ok(list) => {
                let expr = match list.as_slice() {
                    [one] => format!("status = {}", show(one)),
                    many => format!("status ∈ [{}]", many.iter().map(show).collect::<Vec<_>>().join(", ")),
                };
                let pass = list.contains(&actual.status);
                // gRPC 错误状态带上服务端给的说明（grpc-message），不用再去翻元数据
                let why = actual.headers.iter().find(|(k, _)| actual.protocol == Protocol::Grpc && k == "grpc-message").map(|(_, v)| format!("：{v}")).unwrap_or_default();
                out.push(outcome(expr, (!pass).then(|| format!("实际 {}{why}", show(&actual.status)))));
            }
            Err(e) => out.push(outcome(format!("status = {}", match st {
                ExpectStatus::Var(t) => t.clone(),
                ExpectStatus::Names(n) => n.join(", "),
                ExpectStatus::One(c) => c.to_string(),
                ExpectStatus::Many(v) => format!("{v:?}"),
            }), Some(e))),
        }
    }
    for (name, want) in &expect.headers {
        let want = render(want, vars);
        let values: Vec<&str> = actual.headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str()).collect();
        let got = (!values.is_empty()).then(|| Value::String(values.join(", ")));
        let detail = check(got.as_ref(), &want, false).err();
        out.push(outcome(format!("header {name} {}", describe(&want)), detail));
    }
    if expect.body.is_empty() {
        return out;
    }
    let parsed: Option<Value> = serde_json::from_str(actual.body).ok();
    for (path, raw) in &expect.body {
        let want = render(raw, vars);
        let loose = from_text_var(raw, vars);
        let expr = format!("body{} {}", display_path(path), describe(&want));
        let root = match &parsed {
            Some(v) => v.clone(),
            None if is_root(path) => Value::String(actual.body.to_string()),
            None => {
                out.push(outcome(expr, Some(format!("响应体不是 JSON：{}", clip(actual.body, 80)))));
                continue;
            }
        };
        out.push(outcome(expr, check_path(&root, path, &want, loose).err()));
    }
    out
}

/// 一组「路径 → 期望」的简短描述（WebSocket 步骤标题用）：`type = "joined"，data.room = 1`
pub fn describe_matchers(m: &Map<String, Value>, vars: &Vars) -> String {
    m.iter()
        .map(|(path, want)| {
            let p = trim_root(path);
            let p = if p.is_empty() { "$" } else { p };
            format!("{p} {}", describe(&render(want, vars)))
        })
        .collect::<Vec<_>>()
        .join("，")
}

/// 捕获结果：成功的变量，以及取不到值的失败断言
pub struct Captured {
    pub vars: Vec<(String, VarVal)>,
    pub failures: Vec<TestOutcome>,
}

/// 按声明从响应中取值写入运行时变量。来源：响应体路径（缺省）或 `header:名称`
pub fn capture(spec: &Map<String, Value>, actual: &Actual<'_>) -> Captured {
    let mut res = Captured { vars: vec![], failures: vec![] };
    let parsed: Option<Value> = serde_json::from_str(actual.body).ok();
    for (name, from) in spec {
        let Some(from) = from.as_str() else {
            res.failures.push(outcome(format!("capture {name}"), Some("来源必须是字符串：响应体路径或 header:名称".into())));
            continue;
        };
        let expr = format!("capture {name} ← {from}");
        if let Some(h) = from.strip_prefix("header:") {
            match actual.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(h.trim())) {
                Some((_, v)) => res.vars.push((name.clone(), VarVal::Str(v.clone()))),
                None => res.failures.push(outcome(expr, Some(format!("响应中没有响应头 {}", h.trim())))),
            }
            continue;
        }
        let Some(root) = &parsed else {
            res.failures.push(outcome(expr, Some(format!("响应体不是 JSON：{}", clip(actual.body, 80)))));
            continue;
        };
        match select(root, from) {
            Ok(sel) if sel.wildcard => res.failures.push(outcome(expr, Some("捕获路径不能包含 [*]：请写出具体下标".into()))),
            Ok(sel) => match sel.items.first().and_then(|(_, v)| *v) {
                Some(v) => res.vars.push((name.clone(), to_var(v))),
                None => res.failures.push(outcome(expr, Some(format!("响应中没有 {}", trim_root(from))))),
            },
            Err(e) => res.failures.push(outcome(expr, Some(e))),
        }
    }
    res
}

/// 校验期望值本身的写法（validate 用）：返回问题描述
pub fn lint_matcher(want: &Value) -> Vec<String> {
    let Value::Object(m) = want else { return vec![] };
    if !is_operator_object(want) {
        // 混了运算符名和普通字段：多半是拼错了运算符
        let unknown: Vec<&String> = m.keys().filter(|k| !OPERATORS.contains(&k.as_str())).collect();
        if unknown.len() < m.len() {
            return vec![format!(
                "运算符对象里有未知运算符 {}（可用：{}）",
                unknown.iter().map(|k| k.as_str()).collect::<Vec<_>>().join("、"),
                OPERATORS.join(" / ")
            )];
        }
        // 只有一个拼错的运算符（{"lenn": 1}、{"startswith": "x"}）：会被当成整体比较对象，运行时才失败
        let near: Vec<String> = m.keys().filter_map(|k| near_operator(k).map(|op| format!("{k} → {op}"))).collect();
        if !near.is_empty() {
            return vec![format!("{} 不是运算符，是不是想写 {}？要整体比较对象请写 {{\"eq\": {{…}}}}", near.iter().map(|n| n.split(" → ").next().unwrap_or_default()).collect::<Vec<_>>().join("、"), near.iter().map(|n| n.split(" → ").nth(1).unwrap_or_default()).collect::<Vec<_>>().join("、"))];
        }
        return vec![];
    }
    let mut issues = vec![];
    for (op, arg) in m {
        let ok = match op.as_str() {
            "gt" | "gte" | "lt" | "lte" => arg.is_number() || is_placeholder(arg),
            "type" => arg.as_str().is_some_and(|t| TYPES.contains(&t)),
            "exists" => arg.is_boolean(),
            "startsWith" | "endsWith" => arg.is_string(),
            "matches" => arg.as_str().is_some_and(|r| regex::Regex::new(r).is_ok()),
            "len" => arg.is_number() || is_operator_object(arg) || is_placeholder(arg),
            "oneOf" => arg.is_array(),
            "values" => arg.is_array() || arg.is_string(),
            _ => true,
        };
        if !ok {
            issues.push(match op.as_str() {
                "type" => format!("type 只能是 {}", TYPES.join(" / ")),
                "matches" => format!("matches 不是合法的正则：{arg}"),
                _ => format!("{op} 的参数不合法：{arg}"),
            });
        }
    }
    issues
}

/// 与某个运算符只差大小写或一两个字母的键（拼写错误）
fn near_operator(key: &str) -> Option<&'static str> {
    fn distance(a: &str, b: &str) -> usize {
        let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
        let mut prev: Vec<usize> = (0..=b.len()).collect();
        for (i, ca) in a.iter().enumerate() {
            let mut cur = vec![i + 1];
            for (j, cb) in b.iter().enumerate() {
                cur.push((prev[j] + usize::from(ca != cb)).min(prev[j + 1] + 1).min(cur[j] + 1));
            }
            prev = cur;
        }
        prev[b.len()]
    }
    let k = key.to_lowercase();
    // 常见的普通字段名，不当成拼写错误
    if matches!(k.as_str(), "value" | "values_" | "name" | "names" | "types" | "id" | "ids" | "length" | "size") && !OPERATORS.contains(&key) {
        return None;
    }
    OPERATORS.iter().copied().find(|op| {
        let o = op.to_lowercase();
        k == o || (o.len() >= 3 && k.len() >= 3 && distance(&k, &o) <= if o.len() >= 6 { 2 } else { 1 })
    })
}

/// 路径写法是否合法（validate 用）
pub fn lint_path(path: &str) -> Option<String> {
    parse_path(path).err()
}

// ---------------------------------------------------------------- 路径

#[derive(Debug, Clone, PartialEq)]
enum Seg {
    Key(String),
    Index(usize),
    All,
}

struct Selection<'a> {
    /// (具体路径, 值)；None = 该位置没有这个字段（[*] 展开后的元素缺字段同样保留，不会被悄悄跳过）
    items: Vec<(String, Option<&'a Value>)>,
    wildcard: bool,
}

fn is_root(path: &str) -> bool {
    matches!(path.trim(), "" | "$")
}

fn trim_root(path: &str) -> &str {
    let p = path.trim();
    p.strip_prefix("$.").or_else(|| p.strip_prefix('$')).unwrap_or(p)
}

fn display_path(path: &str) -> String {
    match trim_root(path) {
        "" => String::new(),
        p if p.starts_with('[') => p.to_string(),
        p => format!(".{p}"),
    }
}

fn parse_path(path: &str) -> Result<Vec<Seg>, String> {
    let p = trim_root(path);
    let mut segs = vec![];
    if p.is_empty() {
        return Ok(segs);
    }
    for part in p.split('.') {
        if part.is_empty() {
            return Err(format!("路径 {path} 有空段（连续的 . ）"));
        }
        let (head, mut rest) = match part.find('[') {
            Some(i) => (&part[..i], &part[i..]),
            None => (part, ""),
        };
        match head {
            "" => {}
            "*" => segs.push(Seg::All),
            h => segs.push(match h.parse::<usize>() {
                Ok(n) => Seg::Index(n),
                Err(_) => Seg::Key(h.to_string()),
            }),
        }
        while !rest.is_empty() {
            let close = rest.find(']').ok_or_else(|| format!("路径 {path} 的 [ 没有闭合"))?;
            let inner = rest[1..close].trim();
            segs.push(match inner {
                "*" => Seg::All,
                n => Seg::Index(n.parse().map_err(|_| format!("路径 {path} 的 [{n}] 不是下标（用 [0] / [*]）"))?),
            });
            rest = &rest[close + 1..];
            if !rest.is_empty() && !rest.starts_with('[') {
                return Err(format!("路径 {path} 的 ] 后面应接 . 或 ["));
            }
        }
    }
    Ok(segs)
}

fn select<'a>(root: &'a Value, path: &str) -> Result<Selection<'a>, String> {
    let segs = parse_path(path)?;
    let wildcard = segs.contains(&Seg::All);
    let mut cur: Vec<(String, Option<&Value>)> = vec![(String::new(), Some(root))];
    for seg in &segs {
        let mut next = vec![];
        for (at, v) in cur {
            match (seg, v) {
                (Seg::Key(k), v) => next.push((format!("{at}.{k}"), v.and_then(|v| v.as_object()).and_then(|m| m.get(k)))),
                (Seg::Index(i), v) => next.push((
                    format!("{at}[{i}]"),
                    v.and_then(|v| match v {
                        Value::Array(a) => a.get(*i),
                        Value::Object(m) => m.get(&i.to_string()),
                        _ => None,
                    }),
                )),
                (Seg::All, Some(Value::Array(a))) => next.extend(a.iter().enumerate().map(|(i, x)| (format!("{at}[{i}]"), Some(x)))),
                (Seg::All, Some(Value::Object(m))) => next.extend(m.iter().map(|(k, x)| (format!("{at}.{k}"), Some(x)))),
                (Seg::All, _) => next.push((format!("{at}[*]"), None)),
            }
        }
        cur = next;
    }
    Ok(Selection { items: cur, wildcard })
}

fn check_path(root: &Value, path: &str, want: &Value, loose: bool) -> Result<(), String> {
    let sel = select(root, path)?;
    if !sel.wildcard {
        return check(sel.items.first().and_then(|(_, v)| *v), want, loose);
    }
    // values：比较收集到的整列（按顺序）；同一对象里的其他运算符仍逐个元素检查
    if let Some(expected) = want.as_object().filter(|_| is_operator_object(want)).and_then(|m| m.get("values")) {
        let got: Vec<Option<&Value>> = sel.items.iter().map(|(_, v)| *v).collect();
        values_eq(&got, expected)?;
        let mut rest = want.as_object().cloned().unwrap_or_default();
        rest.remove("values");
        if rest.is_empty() {
            return Ok(());
        }
        return check_path(root, path, &Value::Object(rest), loose);
    }
    // [*]：每个元素都要满足；一个都没有视为失败（除非期望不存在）
    if sel.items.is_empty() {
        return match want_absent(want) {
            true => Ok(()),
            false => Err(format!("没有匹配 {} 的元素（数组为空）", trim_root(path))),
        };
    }
    for (at, v) in &sel.items {
        if let Err(e) = check(*v, want, loose) {
            return Err(format!("{}：{e}（共 {} 个）", at.trim_start_matches('.'), sel.items.len()));
        }
    }
    Ok(())
}

/// 收集到的值按顺序等于期望：期望写数组，或逗号分隔的文本（如数据行里的 "101,102,103"）
fn values_eq(got: &[Option<&Value>], expected: &Value) -> Result<(), String> {
    let text = |v: &Value| match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let ok = match expected {
        Value::Array(want) => want.len() == got.len() && want.iter().zip(got).all(|(w, g)| g.is_some_and(|g| json_eq(g, w))),
        Value::String(s) => {
            let want: Vec<&str> = if s.trim().is_empty() { vec![] } else { s.split(',').map(str::trim).collect() };
            want.len() == got.len() && want.iter().zip(got).all(|(w, g)| g.is_some_and(|g| text(g) == *w))
        }
        _ => return Err("values 的参数是数组，或逗号分隔的文本".into()),
    };
    if ok {
        return Ok(());
    }
    let shown: Vec<String> = got.iter().map(|g| g.map(|v| v.to_string()).unwrap_or_else(|| "（缺失）".into())).collect();
    // 逗号文本按列表展示，与实际值同一格式，便于逐个对照
    let want = match expected {
        Value::String(s) => format!("[{}]", s.split(',').map(str::trim).filter(|x| !x.is_empty()).collect::<Vec<_>>().join(", ")),
        other => show(other),
    };
    Err(format!("期望依次为 {}，实际 [{}]（共 {} 个）", clip(&want, 160), clip(&shown.join(", "), 160), got.len()))
}

fn want_absent(want: &Value) -> bool {
    matches!(want, Value::Object(m) if is_operator_object(want) && m.get("exists") == Some(&Value::Bool(false)))
}

// ---------------------------------------------------------------- 匹配

/// 期望值整个是 `{{变量}}` 且变量是文本（环境文件 / CSV 的变量都是文本）：与数字 / 布尔按文本比较，
/// 这样 `"data.uid": "{{uid}}"` 不用关心变量来自哪一层；直接写在文件里的 "42" 仍严格区分类型
fn from_text_var(raw: &Value, vars: &Vars) -> bool {
    matches!(raw, Value::String(s) if single_placeholder(s).is_some_and(|(name, tag)| tag.is_none() && matches!(vars.get(name), Some(VarVal::Str(_)))))
}

fn text_eq(a: &Value, want: &Value) -> bool {
    match (a, want) {
        (Value::Number(n), Value::String(s)) => s.trim().parse::<f64>().ok().zip(n.as_f64()).is_some_and(|(x, y)| x == y),
        (Value::Bool(b), Value::String(s)) => s.trim() == b.to_string(),
        _ => false,
    }
}

fn check(actual: Option<&Value>, want: &Value, loose: bool) -> Result<(), String> {
    if !is_operator_object(want) {
        return match actual {
            None => Err(format!("字段不存在（期望 {}）", show(want))),
            Some(a) if json_eq(a, want) || (loose && text_eq(a, want)) => Ok(()),
            Some(a) => Err(mismatch(want, a)),
        };
    }
    let ops = want.as_object().expect("运算符对象");
    if let Some(Value::Bool(should)) = ops.get("exists") {
        return match (actual.is_some(), *should) {
            (true, true) | (false, false) => match actual {
                Some(a) => check_ops(a, ops),
                None => Ok(()),
            },
            (false, true) => Err("字段不存在".into()),
            (true, false) => Err(format!("期望不存在，实际 {}", show(actual.unwrap_or(&Value::Null)))),
        };
    }
    match actual {
        None => Err(format!("字段不存在（期望 {}）", describe(want))),
        Some(a) => check_ops(a, ops),
    }
}

fn check_ops(a: &Value, ops: &Map<String, Value>) -> Result<(), String> {
    let mut errs = vec![];
    for (op, arg) in ops {
        if let Err(e) = check_op(a, op, arg) {
            errs.push(e);
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("；"))
    }
}

fn check_op(a: &Value, op: &str, arg: &Value) -> Result<(), String> {
    let num = |v: &Value| v.as_f64();
    // 参数来自文本变量（"{{min}}" → "3"）时按数字理解
    let arg_num = |v: &Value| v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()));
    let fail = |what: String| Err(format!("{what}，实际 {}", show(a)));
    match op {
        "exists" => Ok(()),
        "eq" => if json_eq(a, arg) { Ok(()) } else { Err(mismatch(arg, a)) },
        "ne" => if json_eq(a, arg) { fail(format!("期望不等于 {}", show(arg))) } else { Ok(()) },
        "gt" | "gte" | "lt" | "lte" => match (num(a), arg_num(arg)) {
            (Some(x), Some(y)) => {
                let ok = match op {
                    "gt" => x > y,
                    "gte" => x >= y,
                    "lt" => x < y,
                    _ => x <= y,
                };
                if ok { Ok(()) } else { fail(format!("期望 {} {}", op_symbol(op), show(arg))) }
            }
            (None, _) => fail(format!("期望数字 {} {}", op_symbol(op), show(arg))),
            (_, None) => Err(format!("{op} 的参数不是数字：{}", show(arg))),
        },
        "type" => {
            let t = arg.as_str().unwrap_or_default();
            let ok = match t {
                "string" => a.is_string(),
                "number" => a.is_number(),
                "integer" => a.as_f64().is_some_and(|f| f.fract() == 0.0),
                "boolean" => a.is_boolean(),
                "null" => a.is_null(),
                "array" => a.is_array(),
                "object" => a.is_object(),
                _ => return Err(format!("type 只能是 {}", TYPES.join(" / "))),
            };
            if ok { Ok(()) } else { fail(format!("期望类型 {t}（实际类型 {}）", type_name(a))) }
        }
        "contains" => {
            let ok = match (a, arg) {
                (Value::String(s), Value::String(sub)) => s.contains(sub.as_str()),
                // 文本里包含数字 / 布尔：按其文本比较（表单里写「包含 3」不必加引号）
                (Value::String(s), x @ (Value::Number(_) | Value::Bool(_))) => s.contains(x.to_string().as_str()),
                (Value::Array(items), x) => items.iter().any(|i| json_eq(i, x)),
                (Value::Object(m), Value::String(k)) => m.contains_key(k),
                _ => false,
            };
            if ok { Ok(()) } else { fail(format!("期望包含 {}", show(arg))) }
        }
        "startsWith" | "endsWith" => match (a.as_str(), arg.as_str()) {
            (Some(s), Some(p)) => {
                let ok = if op == "startsWith" { s.starts_with(p) } else { s.ends_with(p) };
                if ok { Ok(()) } else { fail(format!("期望{} {}", if op == "startsWith" { "以此开头" } else { "以此结尾" }, show(arg))) }
            }
            _ => fail(format!("期望字符串 {op} {}", show(arg))),
        },
        "matches" => {
            let pattern = arg.as_str().unwrap_or_default();
            let re = regex::Regex::new(pattern).map_err(|e| format!("matches 不是合法的正则：{e}"))?;
            match a.as_str() {
                Some(s) if re.is_match(s) => Ok(()),
                _ => fail(format!("期望匹配正则 {pattern}")),
            }
        }
        "len" => {
            let n = match a {
                Value::String(s) => s.chars().count(),
                Value::Array(x) => x.len(),
                Value::Object(m) => m.len(),
                _ => return fail("期望有长度（字符串 / 数组 / 对象）".into()),
            };
            let n = Value::from(n);
            match arg {
                x if is_operator_object(x) => check(Some(&n), x, false).map_err(|e| format!("长度不符：{e}")),
                x => if json_eq(&n, x) || text_eq(&n, x) { Ok(()) } else { Err(format!("期望长度 {}，实际长度 {n}", show(x))) },
            }
        }
        "values" => {
            let items: Vec<Option<&Value>> = match a {
                Value::Array(x) => x.iter().map(Some).collect(),
                other => vec![Some(other)],
            };
            values_eq(&items, arg)
        }
        "oneOf" => match arg.as_array() {
            Some(opts) if opts.iter().any(|o| json_eq(a, o)) => Ok(()),
            Some(_) => fail(format!("期望是 {} 之一", show(arg))),
            None => Err("oneOf 的参数必须是数组".into()),
        },
        other => Err(format!("未知运算符 {other}")),
    }
}

fn op_symbol(op: &str) -> &'static str {
    match op {
        "gt" => ">",
        "gte" => "≥",
        "lt" => "<",
        _ => "≤",
    }
}

/// 相等：数字按数值比较（1 与 1.0 相等），其余结构递归比较
fn json_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_i64(), y.as_i64(), x.as_u64(), y.as_u64()) {
            (Some(i), Some(j), _, _) => i == j,
            (_, _, Some(i), Some(j)) => i == j,
            _ => x.as_f64() == y.as_f64(),
        },
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(i, j)| json_eq(i, j)),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| json_eq(v, w))),
        _ => a == b,
    }
}

fn mismatch(want: &Value, got: &Value) -> String {
    let same_text = matches!((want, got), (Value::String(s), Value::Number(_) | Value::Bool(_)) | (Value::Number(_) | Value::Bool(_), Value::String(s)) if *s == value_text(if want.is_string() { got } else { want }));
    if same_text {
        format!("期望 {}（{}），实际 {}（{}）：类型不同", show(want), type_name(want), show(got), type_name(got))
    } else {
        format!("期望 {}，实际 {}", show(want), show(got))
    }
}

fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::String(_) => "字符串",
        Value::Number(_) => "数字",
        Value::Bool(_) => "布尔",
        Value::Null => "null",
        Value::Array(_) => "数组",
        Value::Object(_) => "对象",
    }
}

/// 期望值的可读描述：`= 0` / `> 0 · len 2`
pub fn describe(want: &Value) -> String {
    if !is_operator_object(want) {
        return format!("= {}", show(want));
    }
    want.as_object()
        .expect("运算符对象")
        .iter()
        .map(|(op, arg)| match op.as_str() {
            "eq" => format!("= {}", show(arg)),
            "ne" => format!("≠ {}", show(arg)),
            "gt" | "gte" | "lt" | "lte" => format!("{} {}", op_symbol(op), show(arg)),
            "exists" => if arg == &Value::Bool(false) { "不存在".into() } else { "存在".into() },
            "len" if is_operator_object(arg) => format!("len {}", describe(arg)),
            "values" => format!("依次为 {}", show(arg)),
            other => format!("{other} {}", show(arg)),
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

fn show(v: &Value) -> String {
    clip(&v.to_string(), 120)
}

fn clip(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    format!("{}…", s.chars().take(max).collect::<String>())
}

fn outcome(expr: String, detail: Option<String>) -> TestOutcome {
    TestOutcome { expr, pass: detail.is_none(), detail }
}

// ---------------------------------------------------------------- 变量

fn is_placeholder(v: &Value) -> bool {
    v.as_str().is_some_and(|s| single_placeholder(s).is_some())
}

/// 整个字符串恰好是一个 `{{name}}` / `{{name:type}}` → (name, type)
fn single_placeholder(s: &str) -> Option<(&str, Option<&str>)> {
    let inner = s.trim().strip_prefix("{{")?.strip_suffix("}}")?;
    if inner.contains("{{") || inner.contains("}}") || inner.trim_start().starts_with('%') {
        return None;
    }
    let inner = inner.trim();
    Some(match inner.split_once(':') {
        Some((n, t)) => (n.trim(), Some(t.trim())),
        None => (inner, None),
    })
}

/// 渲染期望值中的变量：整值占位符保留原类型（`:number` / `:bool` 强制转换），其余字符串按文本渲染
fn render(v: &Value, vars: &Vars) -> Value {
    match v {
        Value::String(s) => {
            if let Some((name, tag)) = single_placeholder(s) {
                if let Some(val) = vars.get(name) {
                    return match (tag, val) {
                        (Some("number"), x) => x.to_render().parse::<f64>().ok().map(num_value).unwrap_or_else(|| Value::String(x.to_render())),
                        (Some("bool"), x) => match x.to_render().as_str() {
                            "true" => Value::Bool(true),
                            "false" => Value::Bool(false),
                            other => Value::String(other.to_string()),
                        },
                        (_, VarVal::Num(n)) => num_value(*n),
                        (_, VarVal::Bool(b)) => Value::Bool(*b),
                        (_, VarVal::Str(x)) => Value::String(x.clone()),
                    };
                }
            }
            Value::String(vars.render(s))
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| render(x, vars)).collect()),
        Value::Object(m) => Value::Object(m.iter().map(|(k, x)| (k.clone(), render(x, vars))).collect()),
        other => other.clone(),
    }
}

fn num_value(n: f64) -> Value {
    if n.fract() == 0.0 && n.abs() < 9.007_199_254_740_992e15 {
        Value::from(n as i64)
    } else {
        serde_json::Number::from_f64(n).map(Value::Number).unwrap_or(Value::Null)
    }
}

/// 捕获值 → 运行时变量：数字 / 布尔保留类型；超出安全整数范围的数字保留原文字符串
fn to_var(v: &Value) -> VarVal {
    match v {
        Value::String(s) => VarVal::Str(s.clone()),
        Value::Bool(b) => VarVal::Bool(*b),
        Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) if i.unsigned_abs() > (1u64 << 53) => VarVal::Str(n.to_string()),
            (None, _) if n.as_u64().is_some() => VarVal::Str(n.to_string()),
            (_, Some(f)) => VarVal::Num(f),
            _ => VarVal::Str(n.to_string()),
        },
        Value::Null => VarVal::Str(String::new()),
        other => VarVal::Str(other.to_string()),
    }
}

/// 期望值 / 捕获来源里引用的变量名（依赖分析与未定义变量检查用）
pub fn referenced_texts(expect: &Expect) -> Vec<String> {
    fn walk(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => out.push(s.clone()),
            Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            Value::Object(m) => m.values().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut out = vec![];
    if let Some(ExpectStatus::Var(t)) = &expect.status {
        out.push(t.clone());
    }
    expect.headers.values().chain(expect.body.values()).for_each(|v| walk(v, &mut out));
    out
}

// ---------------------------------------------------------------- 表单 ↔ 文件（Desktop 断言表格）

/// 表单里的条件：(写入文件的运算符, 界面文字)。`=` 表示直接写字面量（相等），`json` 表示原样编辑整个期望值
pub const FORM_OPS: &[(&str, &str)] = &[
    ("=", "等于"),
    ("ne", "不等于"),
    ("gt", "大于"),
    ("gte", "大于等于"),
    ("lt", "小于"),
    ("lte", "小于等于"),
    ("type", "类型是"),
    ("exists", "存在"),
    ("contains", "包含"),
    ("startsWith", "开头是"),
    ("endsWith", "结尾是"),
    ("matches", "匹配正则"),
    ("len", "长度"),
    ("oneOf", "属于其一"),
    ("values", "依次等于"),
    ("json", "组合（JSON）"),
];

/// 单行 JSON，逗号 / 冒号后带空格（和手写的样子一致）
pub fn inline_json(v: &Value) -> String {
    match v {
        Value::Object(m) => format!(
            "{{{}}}",
            m.iter().map(|(k, x)| format!("{}: {}", Value::String(k.clone()), inline_json(x))).collect::<Vec<_>>().join(", ")
        ),
        Value::Array(a) => format!("[{}]", a.iter().map(inline_json).collect::<Vec<_>>().join(", ")),
        other => other.to_string(),
    }
}

/// 参数按原文处理（不解析 JSON）的运算符
const TEXT_OPS: &[&str] = &["type", "startsWith", "endsWith", "matches"];

/// 表单值文本 → JSON：能按 JSON 解析的（数字 / true / null / "引号字符串" / 数组 / 对象）按 JSON，其余是字符串
pub fn parse_literal(text: &str) -> Value {
    let t = text.trim();
    if t.is_empty() {
        return Value::String(String::new());
    }
    serde_json::from_str(t).unwrap_or_else(|_| Value::String(text.to_string()))
}

/// JSON → 表单值文本（parse_literal 的逆：会被误读成别的类型的字符串才加引号）
pub fn literal_text(v: &Value) -> String {
    match v {
        Value::String(s) if parse_literal(s) == *v => s.clone(),
        other => other.to_string(),
    }
}

/// 文件里的期望值 → 表单 (条件, 值文本)
pub fn split_matcher(want: &Value) -> (&'static str, String) {
    let op_of = |k: &str| FORM_OPS.iter().map(|(op, _)| *op).find(|op| *op == k);
    if let Value::Object(m) = want {
        if is_operator_object(want) {
            if m.len() == 1 {
                let (k, arg) = m.iter().next().expect("one entry");
                if k == "eq" && !is_operator_object(arg) {
                    return ("=", literal_text(arg));
                }
                match (op_of(k), arg) {
                    (Some(op), Value::String(s)) if TEXT_OPS.contains(&op) => return (op, s.clone()),
                    (Some(op), _) if TEXT_OPS.contains(&op) => {}
                    (Some("exists"), Value::Bool(b)) => return ("exists", b.to_string()),
                    (Some("exists"), _) => {}
                    (Some(op), arg) => return (op, literal_text(arg)),
                    (None, _) => {}
                }
            }
            return ("json", inline_json(want));
        }
    }
    ("=", literal_text(want))
}

/// 表单 (条件, 值文本) → 文件里的期望值
pub fn join_matcher(op: &str, text: &str) -> Result<Value, String> {
    let one = |op: &str, v: Value| Value::Object(Map::from_iter([(op.to_string(), v)]));
    Ok(match op {
        "=" => parse_literal(text),
        "json" => serde_json::from_str(text.trim()).map_err(|e| format!("「组合（JSON）」不是合法 JSON：{e}"))?,
        "exists" => match text.trim() {
            "" | "true" | "是" => one(op, Value::Bool(true)),
            "false" | "否" => one(op, Value::Bool(false)),
            t => return Err(format!("「存在」的值写 true 或 false（现在是 {t}）")),
        },
        "type" => {
            let t = text.trim();
            if !TYPES.contains(&t) {
                return Err(format!("「类型是」可选：{}（现在是 {t:?}）", TYPES.join(" / ")));
            }
            one(op, Value::String(t.into()))
        }
        "oneOf" => match parse_literal(text) {
            v @ Value::Array(_) => one(op, v),
            _ => return Err(r#"「属于其一」的值写成 JSON 数组，如 ["a", "b"] 或 [200, 201]"#.into()),
        },
        op if TEXT_OPS.contains(&op) => one(op, Value::String(text.to_string())),
        op if OPERATORS.contains(&op) => one(op, parse_literal(text)),
        other => return Err(format!("未知条件 {other}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn resp(body: &str) -> (Vec<(String, String)>, String) {
        (vec![("Content-Type".into(), "application/json; charset=utf-8".into()), ("X-Trace-Id".into(), "t-9".into())], body.into())
    }

    fn run(expect: Value, body: &str, vars: &Vars) -> Vec<TestOutcome> {
        let e: Expect = serde_json::from_value(expect).unwrap();
        let (headers, body) = resp(body);
        evaluate(&e, &Actual { status: 200, headers: &headers, body: &body, protocol: Protocol::Http }, vars)
    }

    fn failures(out: &[TestOutcome]) -> Vec<String> {
        out.iter().filter(|t| !t.pass).map(|t| format!("{} → {}", t.expr, t.detail.clone().unwrap_or_default())).collect()
    }

    const BODY: &str = r#"{"code":0,"msg":"ok","data":{"total":2,"big":9223372036854775806,"url":"http://h/x?expires=3600",
        "items":[{"id":1,"price":9.5,"tags":["a"]},{"id":2,"price":0,"tags":[]}],"empty":null}}"#;

    #[test]
    fn literals_paths_and_wildcards() {
        let out = run(
            json!({"status": [200, 201], "body": {
                "code": 0, "msg": "ok", "data.total": 2.0, "data.items[0].id": 1, "data.items.1.id": 2,
                "data.items[*].id": {"gt": 0}, "data.empty": null, "data.big": 9223372036854775806u64,
                "$.data.items[0].tags": ["a"]
            }}),
            BODY,
            &Vars::new(),
        );
        assert!(failures(&out).is_empty(), "{:?}", failures(&out));
        assert_eq!(out[0].expr, "status ∈ [200, 201]");
        assert_eq!(out[1].expr, "body.code = 0");
    }

    #[test]
    fn failure_details_name_the_field() {
        let out = run(
            json!({"status": 404, "body": {
                "code": 4030001, "data.items[*].price": {"gt": 0}, "data.nope": 1, "data.total": "2",
                "data.items[*].missing": 1
            }}),
            BODY,
            &Vars::new(),
        );
        let f = failures(&out);
        assert_eq!(f[0], "status = 404 → 实际 200");
        assert_eq!(f[1], "body.code = 4030001 → 期望 4030001，实际 0");
        assert_eq!(f[2], "body.data.items[*].price > 0 → data.items[1].price：期望 > 0，实际 0（共 2 个）");
        assert_eq!(f[3], "body.data.nope = 1 → 字段不存在（期望 1）");
        assert!(f[4].contains("类型不同"), "{}", f[4]);
        assert_eq!(f[5], "body.data.items[*].missing = 1 → data.items[0].missing：字段不存在（期望 1）（共 2 个）");
    }

    #[test]
    fn wildcard_does_not_skip_elements_missing_the_field() {
        let out = run(json!({"body": {"items[*].id": {"gt": 0}}}), r#"{"items":[{"id":1},{"name":"x"}]}"#, &Vars::new());
        assert_eq!(failures(&out), ["body.items[*].id > 0 → items[1].id：字段不存在（期望 > 0）（共 2 个）"]);
        let out = run(json!({"body": {"items[*].id": 1}}), r#"{"items":[]}"#, &Vars::new());
        assert!(failures(&out)[0].contains("数组为空"));
    }

    #[test]
    fn operators() {
        let ok = run(
            json!({"headers": {"content-type": {"contains": "json"}}, "body": {
                "msg": {"type": "string", "len": 2, "oneOf": ["ok", "fine"], "ne": "bad", "matches": "^o"},
                "data.url": {"startsWith": "http://", "endsWith": "3600", "contains": "expires"},
                "data.items": {"len": {"gte": 2}, "type": "array"},
                "data.items[0].tags": {"contains": "a"},
                "data.total": {"gte": 2, "lt": 3, "type": "integer"},
                "data.nope": {"exists": false},
                "data.empty": {"exists": true, "type": "null"},
                "data.items[*].nope": {"exists": false},
                "data": {"contains": "total"}
            }}),
            BODY,
            &Vars::new(),
        );
        assert!(failures(&ok).is_empty(), "{:?}", failures(&ok));
        let bad = run(json!({"body": {"msg": {"len": 3, "type": "number"}, "data.total": {"exists": false}}}), BODY, &Vars::new());
        let f = failures(&bad);
        assert!(f[0].contains("期望长度 3，实际长度 2") && f[0].contains("期望类型 number"), "{f:?}");
        assert!(f[1].contains("期望不存在"), "{f:?}");
    }

    #[test]
    fn operator_like_response_objects_compare_literally() {
        // 普通对象里混有非运算符键 → 整体相等比较，而不是被当成运算符
        let out = run(json!({"body": {"$": {"code": 0, "type": "x"}}}), r#"{"code":0,"type":"x"}"#, &Vars::new());
        assert!(failures(&out).is_empty());
        let out = run(json!({"body": {"$": {"eq": {"type": "x"}}}}), r#"{"type":"x"}"#, &Vars::new());
        assert!(failures(&out).is_empty(), "{:?}", failures(&out));
    }

    #[test]
    fn variables_keep_native_types() {
        let mut vars = Vars::new();
        vars.push_layer(
            [("total".to_string(), VarVal::Num(2.0)), ("csvTotal".to_string(), VarVal::Str("2".into())), ("base".to_string(), VarVal::Str("http://h".into()))]
                .into_iter()
                .collect(),
        );
        let out = run(
            json!({"body": {"data.total": "{{total}}", "data.items": {"len": "{{csvTotal:number}}"}, "data.url": {"startsWith": "{{base}}/"}}}),
            BODY,
            &vars,
        );
        assert!(failures(&out).is_empty(), "{:?}", failures(&out));
        // 文本变量（环境文件 / CSV）与数字按文本比较；运算符参数同样可用；直接写的 "2" 仍严格区分类型
        let out = run(json!({"body": {"data.total": "{{csvTotal}}", "data.items": {"len": "{{csvTotal}}"}, "code": {"lt": "{{csvTotal}}"}}}), BODY, &vars);
        assert!(failures(&out).is_empty(), "{:?}", failures(&out));
        let out = run(json!({"body": {"data.total": "2"}}), BODY, &vars);
        assert!(failures(&out)[0].contains("类型不同"));
        let out = run(json!({"body": {"data.total": "{{base}}"}}), BODY, &vars);
        assert!(failures(&out)[0].contains("期望 \"http://h\""));
    }

    #[test]
    fn status_from_row_variable() {
        let mut vars = Vars::new();
        vars.push_layer([("st".to_string(), VarVal::Num(200.0)), ("many".to_string(), VarVal::Str("201, 200".into())), ("bad".to_string(), VarVal::Str("ok".into()))].into_iter().collect());
        let out = run(json!({"status": "{{st}}"}), BODY, &vars);
        assert_eq!((out[0].expr.as_str(), out[0].pass), ("status = 200", true));
        let out = run(json!({"status": "{{many}}"}), BODY, &vars);
        assert_eq!((out[0].expr.as_str(), out[0].pass), ("status ∈ [201, 200]", true));
        assert_eq!(failures(&run(json!({"status": "{{bad}}"}), BODY, &vars)), ["status = {{bad}} → 状态码变量 {{bad}} 的值不是状态码：ok"]);
        assert_eq!(failures(&run(json!({"status": "{{nope}}"}), BODY, &vars)), ["status = {{nope}} → 状态码变量 {{nope}} 未定义"]);
    }

    /// gRPC：状态按名字或数字写，报告里显示名字；失败时带上服务端的 grpc-message
    #[test]
    fn grpc_status_by_name_or_number() {
        let headers = vec![("grpc-status".to_string(), "5".to_string()), ("grpc-message".to_string(), "order 9 not found".to_string())];
        let eval = |status: Value, code: u16| {
            let e: Expect = serde_json::from_value(json!({ "status": status })).unwrap();
            evaluate(&e, &Actual { status: code, headers: &headers, body: "{}", protocol: Protocol::Grpc }, &Vars::new())
        };
        let ok = eval(json!("NOT_FOUND"), 5);
        assert_eq!((ok[0].expr.as_str(), ok[0].pass), ("status = NOT_FOUND", true));
        assert!(eval(json!(5), 5)[0].pass);
        assert!(eval(json!(["OK", "NOT_FOUND"]), 5)[0].pass);
        let bad = eval(json!("OK"), 5);
        assert_eq!(bad[0].detail.as_deref(), Some("实际 NOT_FOUND：order 9 not found"));
        assert_eq!(eval(json!(200), 0)[0].detail.as_deref(), Some("200 不是gRPC 状态（OK、NOT_FOUND… 或 0–16）"));
        // HTTP 不接受状态名
        let e: Expect = serde_json::from_value(json!({ "status": "OK" })).unwrap();
        let out = evaluate(&e, &Actual { status: 200, headers: &[], body: "", protocol: Protocol::Http }, &Vars::new());
        assert_eq!(out[0].detail.as_deref(), Some("OK 不是状态码"));
    }

    #[test]
    fn values_compares_the_collected_sequence() {
        let body = r#"{"data":{"files":[{"pages":[{"id":101},{"id":102}]},{"pages":[{"id":103}]}],"ids":[1,2],"empty":[]}}"#;
        let mut vars = Vars::new();
        vars.push_layer([("ids".to_string(), VarVal::Str("101,102,103".into()))].into_iter().collect());
        let ok = run(
            json!({"body": {
                "data.files[*].pages[*].id": {"values": [101, 102, 103], "gt": 100},
                "data.files[*].pages[*]": {"values": [{"id": 101}, {"id": 102}, {"id": 103}]},
                "data.ids": {"values": [1, 2]},
                "data.empty[*].id": {"values": []}
            }}),
            body,
            &vars,
        );
        assert!(failures(&ok).is_empty(), "{:?}", failures(&ok));
        let ok = run(json!({"body": {"data.files[*].pages[*].id": {"values": "{{ids}}"}}}), body, &vars);
        assert!(failures(&ok).is_empty(), "{:?}", failures(&ok));
        assert_eq!(
            failures(&run(json!({"body": {"data.files[*].pages[*].id": {"values": [101, 103, 102]}}}), body, &vars)),
            ["body.data.files[*].pages[*].id 依次为 [101,103,102] → 期望依次为 [101,103,102]，实际 [101, 102, 103]（共 3 个）"]
        );
        let f = failures(&run(json!({"body": {"data.files[*].pages[*].id": {"values": "101,103,102"}}}), body, &vars));
        assert!(f[0].ends_with("期望依次为 [101, 103, 102]，实际 [101, 102, 103]（共 3 个）"), "{f:?}");
        let f = failures(&run(json!({"body": {"data.files[*].pages[*].nope": {"values": "1,2,3"}}}), body, &vars));
        assert!(f[0].contains("（缺失）"), "{f:?}");
        assert_eq!(lint_matcher(&json!({"values": 3})).len(), 1);
    }

    #[test]
    fn non_json_body() {
        let out = run(json!({"body": {"$": {"contains": "hello"}, "code": 0}}), "hello world", &Vars::new());
        let f = failures(&out);
        assert_eq!(f.len(), 1);
        assert!(f[0].contains("响应体不是 JSON"));
    }

    #[test]
    fn capture_values() {
        let (headers, body) = resp(BODY);
        let spec: Map<String, Value> = serde_json::from_value(json!({
            "total": "data.total", "big": "data.big", "first": "data.items[0]", "trace": "header:x-trace-id",
            "nope": "data.nope", "all": "data.items[*].id", "h": "header:X-None"
        }))
        .unwrap();
        let c = capture(&spec, &Actual { status: 200, headers: &headers, body: &body, protocol: Protocol::Http });
        let vars: std::collections::BTreeMap<_, _> = c.vars.into_iter().collect();
        assert_eq!(vars["total"], VarVal::Num(2.0));
        assert_eq!(vars["big"], VarVal::Str("9223372036854775806".into()));
        assert_eq!(vars["first"], VarVal::Str(r#"{"id":1,"price":9.5,"tags":["a"]}"#.into()));
        assert_eq!(vars["trace"], VarVal::Str("t-9".into()));
        let f: Vec<String> = c.failures.iter().map(|t| t.detail.clone().unwrap()).collect();
        assert_eq!(f, ["响应中没有 data.nope", "捕获路径不能包含 [*]：请写出具体下标", "响应中没有响应头 X-None"]);
    }

    #[test]
    fn near_miss_operators_are_flagged() {
        for (bad, hint) in [(json!({"lenn": 1}), "len"), (json!({"startswith": "x"}), "startsWith"), (json!({"Exists": true}), "exists"), (json!({"oneof": [1]}), "oneOf"), (json!({"type_": "string"}), "type"), (json!({"valuse": [1]}), "values")] {
            let issues = lint_matcher(&bad);
            assert!(issues.len() == 1 && issues[0].contains(hint), "{bad}: {issues:?}");
        }
        // 正常的对象字面量不误报
        for ok in [json!({"name": "a", "id": 1}), json!({"length": 3}), json!({"value": 1}), json!({"items": []}), json!({"types": ["a"]})] {
            assert!(lint_matcher(&ok).is_empty(), "{ok}");
        }
    }

    #[test]
    fn lints() {
        assert!(lint_matcher(&json!({"gt": 1, "lenn": 2})).iter().any(|e| e.contains("lenn")));
        assert!(lint_matcher(&json!({"type": "str"}))[0].contains("type 只能是"));
        assert!(lint_matcher(&json!({"matches": "("}))[0].contains("正则"));
        assert!(lint_matcher(&json!({"code": 0, "msg": "x"})).is_empty(), "普通对象不是运算符");
        assert!(lint_path("data.items[x]").is_some());
        assert!(lint_path("data..x").is_some());
        assert!(lint_path("data.items[*].id").is_none());
        assert_eq!(describe(&json!({"gt": 0, "len": {"lte": 3}})), "> 0 · len ≤ 3");
    }

    #[test]
    fn form_round_trips() {
        for v in [
            json!(0),
            json!("ok"),
            json!("0"),
            json!("true"),
            json!(""),
            json!("{{uid}}"),
            json!(null),
            json!([1, 2]),
            json!({"a": 1}),
            json!({"gt": 0}),
            json!({"gte": "{{min}}"}),
            json!({"type": "integer"}),
            json!({"exists": false}),
            json!({"contains": "json"}),
            json!({"contains": 3}),
            json!({"startsWith": "{{baseUrl}}/"}),
            json!({"matches": "^\\d+$"}),
            json!({"len": 2}),
            json!({"len": {"gte": 1}}),
            json!({"oneOf": ["a", "b"]}),
            json!({"values": [101, 102]}),
            json!({"values": "{{ids}}"}),
            json!({"values": "101,102"}),
            json!({"gt": 0, "lt": 10}),
            json!({"eq": {"gt": 1}}),
            json!({"type": 1}),
        ] {
            let (op, text) = split_matcher(&v);
            assert_eq!(join_matcher(op, &text).unwrap(), v, "{op} {text}");
        }
        assert_eq!(split_matcher(&json!("0")), ("=", "\"0\"".into()));
        assert_eq!(split_matcher(&json!("ok")), ("=", "ok".into()));
        assert_eq!(split_matcher(&json!({"eq": 5})), ("=", "5".into()));
        assert_eq!(split_matcher(&json!({"gt": 0, "lt": 10})), ("json", r#"{"gt": 0, "lt": 10}"#.into()));
        assert_eq!(join_matcher("=", "  ").unwrap(), json!(""));
        assert_eq!(join_matcher("contains", "Bearer x").unwrap(), json!({"contains": "Bearer x"}));
        assert!(join_matcher("type", "int").unwrap_err().contains("integer"));
        assert!(join_matcher("oneOf", "a").unwrap_err().contains("JSON 数组"));
        assert!(join_matcher("exists", "maybe").is_err());
        assert!(join_matcher("json", "{").is_err());
        // 文本包含数字：「包含 3」对字符串也成立
        assert!(failures(&run(json!({"body": {"data.url": {"contains": 3600}}}), BODY, &Vars::default())).is_empty());
    }
}
