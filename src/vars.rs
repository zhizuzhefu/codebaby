use std::collections::BTreeMap;

/// 四层变量作用域（低 → 高）：
/// project vars < env vars < collection vars < runtime vars
/// 高层覆盖低层；runtime 层由脚本动态写入。
#[derive(Debug, Clone, Default)]
pub struct Vars {
    layers: Vec<BTreeMap<String, String>>,
}

impl Vars {
    pub fn new() -> Self {
        Self { layers: vec![] }
    }

    /// 压入一层（按调用顺序即优先级升序）
    pub fn push_layer(&mut self, vars: BTreeMap<String, String>) {
        self.layers.push(vars);
    }

    /// 从低到高解析，最后命中者胜
    pub fn get(&self, name: &str) -> Option<&String> {
        self.layers.iter().rev().find_map(|l| l.get(name))
    }

    /// 环境快照（注入脚本 env 对象，只读视图：project+env+collection 合并）
    pub fn env_snapshot(&self) -> BTreeMap<String, String> {
        let mut merged = BTreeMap::new();
        for l in &self.layers {
            merged.extend(l.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        merged
    }

    /// 替换文本中的变量占位符（未定义变量原样保留，validate 阶段告警）：
    /// - `{{name}}`        → 字符串替换
    /// - `{{name:number}}` → 数字标注：JSON 值位置 `"{{name:number}}"` 连引号替换为裸数字；
    ///                      裸位置（URL/header）同样替换；值非数字则保留原占位符
    /// - `{{name:bool}}`   → 同上，输出 true/false
    pub fn render(&self, input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let mut rest = input;
        while let Some(start) = rest.find("{{") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            let Some(end) = after.find("}}") else {
                out.push_str("{{");
                rest = after;
                break;
            };
            let inner = after[..end].trim();
            let closing = &after[end + 2..];
            let (name, type_tag) = split_type_tag(inner);
            match self.get(name) {
                Some(v) => match type_tag {
                    Some(tag @ ("number" | "bool")) => {
                        let valid = match tag {
                            "number" => Self::is_number(v),
                            _ => Self::is_bool(v),
                        };
                        if valid {
                            let quoted = out.ends_with('"') && closing.starts_with('"');
                            if quoted {
                                out.pop(); // 去左引号
                            }
                            out.push_str(v);
                            rest = if quoted { &closing[1..] } else { closing }; // 跳右引号
                        } else {
                            out.push_str("{{");
                            out.push_str(inner);
                            out.push_str("}}");
                            rest = closing;
                        }
                    }
                    _ => {
                        out.push_str(v);
                        rest = closing;
                    }
                },
                None => {
                    out.push_str("{{");
                    out.push_str(inner);
                    out.push_str("}}");
                    rest = closing;
                }
            }
        }
        out.push_str(rest);
        out
    }

    fn is_number(s: &str) -> bool {
        s.parse::<f64>().map(|f| f.is_finite()).unwrap_or(false)
    }

    fn is_bool(s: &str) -> bool {
        s == "true" || s == "false"
    }
}

/// 拆分 `name:type` → (name, Some(type))；无 `:` 则 (name, None)
fn split_type_tag(inner: &str) -> (&str, Option<&str>) {
    match inner.find(':') {
        Some(i) => (&inner[..i], Some(inner[i + 1..].trim())),
        None => (inner, None),
    }
}

/// 校验文本中出现的变量占位符是否全部有定义（识别 {{name}} 与 {{name:type}}）
pub fn find_undefined(input: &str, vars: &Vars) -> Vec<String> {
    let mut undefined = vec![];
    let mut rest = input;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        if let Some(end) = after.find("}}") {
            let inner = after[..end].trim();
            let (name, _) = split_type_tag(inner);
            if !name.is_empty() && vars.get(name).is_none() && !undefined.contains(&name.to_string()) {
                undefined.push(name.to_string());
            }
            rest = &after[end + 2..];
        } else {
            break;
        }
    }
    undefined
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> Vars {
        let mut v = Vars::new();
        v.push_layer(BTreeMap::from([("a".into(), "1".into())]));
        v.push_layer(BTreeMap::from([("a".into(), "2".into()), ("b".into(), "x".into())]));
        v
    }

    #[test]
    fn layering_overrides() {
        let v = vars();
        assert_eq!(v.get("a").unwrap(), "2");
        assert_eq!(v.get("b").unwrap(), "x");
    }

    #[test]
    fn render_replaces_and_keeps_undefined() {
        let v = vars();
        assert_eq!(v.render("a={{a}} b={{b}} c={{c}}"), "a=2 b=x c={{c}}");
    }

    #[test]
    fn render_trims_name() {
        let v = vars();
        assert_eq!(v.render("{{ a }}"), "2");
    }

    fn num_vars() -> Vars {
        let mut v = Vars::new();
        v.push_layer(BTreeMap::from([
            ("taskId".into(), "738011164633133".into()),
            ("flag".into(), "true".into()),
            ("notNum".into(), "abc".into()),
        ]));
        v
    }

    #[test]
    fn number_tag_replaces_json_string_position() {
        let v = num_vars();
        // JSON body 序列化后：值带引号 → 连引号替换为裸数字
        assert_eq!(
            v.render(r#"{"taskId":"{{taskId:number}}"}"#),
            r#"{"taskId":738011164633133}"#
        );
    }

    #[test]
    fn number_tag_in_bare_position() {
        let v = num_vars();
        assert_eq!(v.render("x={{taskId:number}}"), "x=738011164633133");
    }

    #[test]
    fn bool_tag_works() {
        let v = num_vars();
        assert_eq!(
            v.render(r#"{"ok":"{{flag:bool}}"}"#),
            r#"{"ok":true}"#
        );
    }

    #[test]
    fn invalid_typed_value_kept_as_is() {
        let v = num_vars();
        // 值非数字：保留占位符（validate/运行报告可见）
        assert_eq!(
            v.render(r#"{"x":"{{notNum:number}}"}"#),
            r#"{"x":"{{notNum:number}}"}"#
        );
    }

    #[test]
    fn undefined_detection_ignores_type_tag() {
        let v = num_vars();
        assert_eq!(find_undefined("{{taskId:number}} {{nope}}", &v), vec!["nope".to_string()]);
    }
}
