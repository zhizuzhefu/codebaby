//! 从真实响应生成断言 / 捕获：把 JSON 响应展开成字段表，每个字段一键变成
//! `expect.body` 里的一条断言或 `capture` 里的一个变量，并写回用例文件。
//!
//! Desktop 响应面板「字段」页与 `requestbaby send --suggest` 共用。
use crate::config::TestCase;
use anyhow::{Context, Result};
use serde_json::{Map, Value, json};
use std::path::Path;

/// 响应 JSON 中的一个字段
#[derive(Debug, Clone, PartialEq)]
pub struct JsonField {
    /// expect / capture 路径（相对响应体根）：`data.items[0].id`
    pub path: String,
    pub value: Value,
}

/// 写回用例的一项：expect.body 的一条断言，或 capture 的一个变量
#[derive(Debug, Clone, PartialEq)]
pub enum Addition {
    Expect { path: String, matcher: Value },
    Capture { name: String, from: String },
}

impl Addition {
    /// 给人看的一行：「断言 data.code = 0」「捕获 token ← data.token」
    pub fn label(&self) -> String {
        match self {
            Addition::Expect { path, matcher } => format!("断言 {path} {}", crate::expect::describe(matcher)),
            Addition::Capture { name, from } => format!("捕获 {name} ← {from}"),
        }
    }

    /// 可直接粘贴进 "body": {…} / "capture": {…} 的一行 JSON
    pub fn snippet(&self) -> String {
        match self {
            Addition::Expect { path, matcher } => format!("{}: {}", Value::String(path.clone()), crate::expect::inline_json(matcher)),
            Addition::Capture { name, from } => format!("{}: {}", Value::String(name.clone()), Value::String(from.clone())),
        }
    }

    /// 写进用例文档：同一路径 / 变量名已存在则覆盖（以最新一次点击为准）
    pub fn apply(&self, doc: &mut Map<String, Value>) {
        let section = |doc: &mut Map<String, Value>, key: &str| -> Map<String, Value> {
            match doc.remove(key) {
                Some(Value::Object(m)) => m,
                _ => Map::new(),
            }
        };
        match self {
            Addition::Expect { path, matcher } => {
                let mut expect = section(doc, "expect");
                let mut body = section(&mut expect, "body");
                body.insert(path.clone(), matcher.clone());
                expect.insert("body".into(), Value::Object(body));
                doc.insert("expect".into(), Value::Object(expect));
            }
            Addition::Capture { name, from } => {
                let mut capture = section(doc, "capture");
                capture.insert(name.clone(), Value::String(from.clone()));
                doc.insert("capture".into(), Value::Object(capture));
            }
        }
    }
}

impl JsonField {
    /// 类型（与 expect 的 type 运算符同名）：string / integer / number / boolean / null / array / object
    pub fn kind(&self) -> &'static str {
        match &self.value {
            Value::String(_) => "string",
            Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
            Value::Number(_) => "number",
            Value::Bool(_) => "boolean",
            Value::Null => "null",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        }
    }

    /// 值预览（单行，长文本截断）
    pub fn preview(&self) -> String {
        let s = match &self.value {
            Value::Array(a) => format!("[…] {} 项", a.len()),
            Value::Object(o) => format!("{{…}} {} 个字段", o.len()),
            v => v.to_string(),
        };
        if s.chars().count() > 120 {
            format!("{}…", s.chars().take(120).collect::<String>())
        } else {
            s
        }
    }

    /// 断言值：标量等于当前值；数组断言长度；对象断言是对象
    pub fn value_expect(&self) -> Addition {
        let matcher = match &self.value {
            Value::Array(a) => json!({ "len": a.len() }),
            Value::Object(_) => json!({ "type": "object" }),
            v => v.clone(),
        };
        Addition::Expect { path: self.path.clone(), matcher }
    }

    /// 断言类型（值每次都会变的字段：id / 时间戳 / token）
    pub fn type_expect(&self) -> Addition {
        Addition::Expect { path: self.path.clone(), matcher: json!({ "type": self.kind() }) }
    }

    /// 建议的变量名：路径最后一个字段名
    pub fn var_name(&self) -> String {
        let last = self
            .path
            .rsplit(['.', '['])
            .map(|s| s.trim_end_matches(']'))
            .find(|s| !s.is_empty() && !s.chars().all(|c| c.is_ascii_digit()))
            .unwrap_or("value");
        let name: String = last.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
        if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) || name == "data" {
            "value".into()
        } else {
            name
        }
    }

    /// 捕获为变量
    pub fn capture(&self, var: &str) -> Addition {
        Addition::Capture { name: var.to_string(), from: self.path.clone() }
    }
}

/// 字段名能否写进点号路径（含 . [ ] 的键、* 与空键无法表达，跳过这个字段及其子字段）
fn plain_key(k: &str) -> bool {
    !k.is_empty() && k != "*" && k.trim() == k && !k.contains(['.', '[', ']']) && !k.starts_with('$')
}

/// 展开 JSON 响应为字段（前序遍历；数组只展开前 `max_items` 个元素；总数上限 `limit`）。
/// 响应体不是 JSON 时返回空
pub fn fields(body: &str, max_items: usize, limit: usize) -> Vec<JsonField> {
    let Ok(root) = serde_json::from_str::<Value>(body) else { return vec![] };
    let mut out = vec![];
    walk("", &root, max_items, limit, &mut out);
    out
}

fn walk(path: &str, v: &Value, max_items: usize, limit: usize, out: &mut Vec<JsonField>) {
    if out.len() >= limit {
        return;
    }
    if !path.is_empty() {
        out.push(JsonField { path: path.to_string(), value: v.clone() });
    }
    match v {
        Value::Object(o) => {
            for (k, x) in o.iter().filter(|(k, _)| plain_key(k)) {
                let p = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
                walk(&p, x, max_items, limit, out);
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter().take(max_items).enumerate() {
                walk(&format!("{path}[{i}]"), x, max_items, limit, out);
            }
        }
        _ => {}
    }
}

/// 给用例追加断言 / 捕获（同一路径 / 变量名覆盖），按规范格式写回
pub fn append_to_case(case_path: &Path, additions: &[Addition]) -> Result<()> {
    let text = std::fs::read_to_string(case_path).with_context(|| format!("读取失败: {}", case_path.display()))?;
    let mut doc: Value = serde_json::from_str(&text).with_context(|| format!("JSON 解析失败: {}", case_path.display()))?;
    let obj = doc.as_object_mut().context("用例不是 JSON 对象")?;
    for a in additions {
        a.apply(obj);
    }
    // 写回前确认仍是合法用例
    serde_json::from_value::<TestCase>(doc.clone()).context("追加后用例不合法")?;
    crate::fsx::write(case_path, crate::fmt::canonical_case_text(&doc)).with_context(|| format!("写入失败: {}", case_path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_paths_and_suggestions() {
        let f = fields(r#"{"code":0,"msg":"ok","data":{"items":[{"id":7,"tag-name":"x","a.b":1}],"token":null,"rate":0.5}}"#, 3, 100);
        let paths: Vec<&str> = f.iter().map(|x| x.path.as_str()).collect();
        assert_eq!(paths, ["code", "msg", "data", "data.items", "data.items[0]", "data.items[0].id", "data.items[0].tag-name", "data.token", "data.rate"]);
        assert_eq!(f[0].value_expect().snippet(), r#""code": 0"#);
        assert_eq!(f[1].value_expect().snippet(), r#""msg": "ok""#);
        assert_eq!(f[3].value_expect().snippet(), r#""data.items": {"len": 1}"#);
        assert_eq!(f[2].value_expect().snippet(), r#""data": {"type": "object"}"#);
        assert_eq!(f[5].type_expect().snippet(), r#""data.items[0].id": {"type": "integer"}"#);
        assert_eq!(f[8].type_expect().snippet(), r#""data.rate": {"type": "number"}"#);
        assert_eq!(f[5].var_name(), "id");
        assert_eq!(f[6].var_name(), "tagname");
        assert_eq!(f[5].capture("orderId").snippet(), r#""orderId": "data.items[0].id""#);
        assert_eq!(f[5].capture("orderId").label(), "捕获 orderId ← data.items[0].id");
        assert_eq!(f[0].value_expect().label(), "断言 code = 0");
        assert!(fields("not json", 3, 100).is_empty());
        let top = fields("[{\"id\":1}]", 3, 100);
        assert_eq!(top.iter().map(|x| x.path.as_str()).collect::<Vec<_>>(), ["[0]", "[0].id"]);
    }

    /// 生成的断言 / 捕获对同一响应必定成立（用真实的 expect 求值器验证）
    #[test]
    fn suggestions_pass_against_the_response() {
        let body = r#"{"code":0,"msg":"成功","list":[1,2],"user":{"name":"O'Brien","vip":true,"note":null,"score":9.5},"a b":1,"0":"zero"}"#;
        let f = fields(body, 5, 100);
        let mut doc = Map::new();
        for x in &f {
            x.value_expect().apply(&mut doc);
        }
        let mut types = Map::new();
        for x in &f {
            x.type_expect().apply(&mut types);
        }
        for x in f.iter().filter(|x| !x.value.is_object() && !x.value.is_array()) {
            x.capture(&x.var_name()).apply(&mut doc);
        }
        let actual = crate::expect::Actual { status: 200, headers: &[], body, protocol: crate::config::Protocol::Http };
        for d in [&doc, &types] {
            let e: crate::expect::Expect = serde_json::from_value(d["expect"].clone()).unwrap();
            let out = crate::expect::evaluate(&e, &actual, &Default::default());
            assert_eq!(out.len(), f.len());
            for r in out {
                assert!(r.pass, "{}: {:?}", r.expr, r.detail);
            }
        }
        let captured = crate::expect::capture(doc["capture"].as_object().unwrap(), &actual);
        assert!(captured.failures.is_empty(), "{:?}", captured.failures);
        assert!(captured.vars.iter().any(|(k, v)| k == "name" && v.to_render() == "O'Brien"));
    }

    #[test]
    fn append_overwrites_same_path_and_keeps_case_valid() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("01-a.json");
        std::fs::write(&p, r#"{"request":{"method":"GET","url":"http://x"},"expect":{"status":200,"body":{"msg":"ok"}},"post":"vars.a = 1"}"#).unwrap();
        let f = fields(r#"{"code":0,"data":{"id":7}}"#, 3, 100);
        append_to_case(&p, &[f[0].value_expect(), f[2].capture("id")]).unwrap();
        append_to_case(&p, &[f[0].type_expect()]).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v["expect"], json!({"status": 200, "body": {"msg": "ok", "code": {"type": "integer"}}}));
        assert_eq!(v["capture"], json!({"id": "data.id"}));
        assert_eq!(v["post"], "vars.a = 1", "已有脚本原样保留");
    }
}
