use std::collections::BTreeMap;

/// 变量值：保留原生类型（脚本捕获的 number/bool 在注入与断言时还原，
/// 渲染 {{x}} 时按字符串展开）
#[derive(Debug, Clone, PartialEq)]
pub enum VarVal {
    Str(String),
    Num(f64),
    Bool(bool),
}

impl VarVal {
    pub fn to_render(&self) -> String {
        match self {
            VarVal::Str(s) => s.clone(),
            VarVal::Num(n) => fmt_num(*n),
            VarVal::Bool(b) => b.to_string(),
        }
    }

    /// JS 字面量（注入脚本环境）
    pub fn to_js_literal(&self) -> String {
        match self {
            VarVal::Str(s) => serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into()),
            VarVal::Num(n) => fmt_num(*n),
            VarVal::Bool(b) => b.to_string(),
        }
    }
}

fn fmt_num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 9.007_199_254_740_992e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// 四层变量作用域（低 → 高）：
/// project vars < env vars < collection vars < runtime vars
/// 高层覆盖低层；runtime 层由脚本动态写入（保留原生类型）。
#[derive(Debug, Clone, Default)]
pub struct Vars {
    layers: Vec<BTreeMap<String, VarVal>>,
}

impl Vars {
    pub fn new() -> Self {
        Self { layers: vec![] }
    }

    /// 压入一层（按调用顺序即优先级升序）
    pub fn push_layer(&mut self, vars: BTreeMap<String, VarVal>) {
        self.layers.push(vars);
    }

    /// 压入一层纯字符串（配置层便捷方法）
    pub fn push_str_layer<I: IntoIterator<Item = (String, String)>>(&mut self, vars: I) {
        self.layers
            .push(vars.into_iter().map(|(k, v)| (k, VarVal::Str(v))).collect());
    }

    /// 从低到高解析，最后命中者胜
    pub fn get(&self, name: &str) -> Option<&VarVal> {
        self.layers.iter().rev().find_map(|l| l.get(name))
    }

    /// 环境快照（注入脚本 env/vars 对象，保留原生类型）
    pub fn env_snapshot(&self) -> BTreeMap<String, VarVal> {
        let mut merged = BTreeMap::new();
        for l in &self.layers {
            merged.extend(l.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        merged
    }

    /// 替换文本中的变量占位符（未定义变量原样保留，validate 阶段告警）：
    /// - `{{name}}`        → 字符串替换
    /// - `{{%NAME}}`       → 读进程环境变量 NAME（CI 密钥不落盘）
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

            // {{%ENV_NAME}}：进程环境变量
            if let Some(env_name) = inner.strip_prefix('%') {
                if !env_name.is_empty() {
                    match std::env::var(env_name) {
                        Ok(v) => {
                            out.push_str(&v);
                            rest = closing;
                            continue;
                        }
                        Err(_) => {
                            out.push_str("{{");
                            out.push_str(inner);
                            out.push_str("}}");
                            rest = closing;
                            continue;
                        }
                    }
                }
            }

            let (name, type_tag) = split_type_tag(inner);
            match self.get(name).map(VarVal::to_render) {
                Some(v) => match type_tag {
                    Some(tag @ ("number" | "bool")) => {
                        let valid = match tag {
                            "number" => v.parse::<f64>().map(|f| f.is_finite()).unwrap_or(false),
                            _ => v == "true" || v == "false",
                        };
                        if valid {
                            let quoted = out.ends_with('"') && closing.starts_with('"');
                            if quoted {
                                out.pop(); // 去左引号
                            }
                            out.push_str(&v);
                            rest = if quoted { &closing[1..] } else { closing }; // 跳右引号
                        } else {
                            out.push_str("{{");
                            out.push_str(inner);
                            out.push_str("}}");
                            rest = closing;
                        }
                    }
                    _ => {
                        out.push_str(&v);
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
}

/// 拆分 `name:type` → (name, Some(type))；无 `:` 则 (name, None)
fn split_type_tag(inner: &str) -> (&str, Option<&str>) {
    match inner.find(':') {
        Some(i) => (&inner[..i], Some(inner[i + 1..].trim())),
        None => (inner, None),
    }
}

/// 校验文本中出现的变量占位符是否全部有定义
/// （{{%ENV}} 跳过：进程环境变量无法静态校验；{{name:type}} 取 name 部分检查）
pub fn find_undefined(input: &str, vars: &Vars) -> Vec<String> {
    let mut undefined = vec![];
    let mut rest = input;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        if let Some(end) = after.find("}}") {
            let inner = after[..end].trim();
            if inner.starts_with('%') {
                rest = &after[end + 2..];
                continue;
            }
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
        v.push_str_layer([
            ("a".into(), "1".into()),
            ("b".into(), "x".into()),
        ]);
        v
    }

    #[test]
    fn layering_overrides() {
        let v = vars();
        assert_eq!(v.get("a"), Some(&VarVal::Str("1".into())));
    }

    #[test]
    fn render_replaces_and_keeps_undefined() {
        let v = vars();
        assert_eq!(v.render("a={{a}} b={{b}} c={{c}}"), "a=1 b=x c={{c}}");
    }

    #[test]
    fn render_trims_name() {
        let v = vars();
        assert_eq!(v.render("{{ a }}"), "1");
    }

    #[test]
    fn native_types_render_and_inject() {
        let mut v = Vars::new();
        let mut layer = BTreeMap::new();
        layer.insert("n".into(), VarVal::Num(200.0));
        layer.insert("flag".into(), VarVal::Bool(true));
        v.push_layer(layer);
        assert_eq!(v.render("{{n}}"), "200");
        assert_eq!(v.get("flag").unwrap().to_js_literal(), "true");
        assert_eq!(v.get("n").unwrap().to_js_literal(), "200");
    }

    #[test]
    fn number_tag_replaces_json_string_position() {
        let v = vars();
        assert_eq!(v.render(r#"{"a":"{{a:number}}"}"#), r#"{"a":1}"#);
    }

    #[test]
    fn undefined_detection_ignores_type_tag() {
        let v = vars();
        assert_eq!(find_undefined("{{a:number}} {{c}}", &v), vec!["c".to_string()]);
    }
}
