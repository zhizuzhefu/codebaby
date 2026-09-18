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

    /// 替换文本中的 {{var}}（未定义变量原样保留，validate 阶段告警）
    pub fn render(&self, input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let mut rest = input;
        while let Some(start) = rest.find("{{") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            match after.find("}}") {
                Some(end) => {
                    let name = after[..end].trim();
                    match self.get(name) {
                        Some(v) => out.push_str(v),
                        None => {
                            out.push_str("{{");
                            out.push_str(name);
                            out.push_str("}}");
                        }
                    }
                    rest = &after[end + 2..];
                }
                None => {
                    out.push_str("{{");
                    rest = after;
                }
            }
        }
        out.push_str(rest);
        out
    }
}

/// 校验文本中出现的 {{var}} 是否全部有定义
pub fn find_undefined(input: &str, vars: &Vars) -> Vec<String> {    let mut undefined = vec![];
    let mut rest = input;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        if let Some(end) = after.find("}}") {
            let name = after[..end].trim().to_string();
            if !name.is_empty() && vars.get(&name).is_none() && !undefined.contains(&name) {
                undefined.push(name);
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
}
