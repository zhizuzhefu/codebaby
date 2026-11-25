//! 写错字段名时的提示：「不认识的字段 expct，是不是想写 expect？」
//!
//! 两处用到：`validate` 的 Schema 错误、`run` 解析用例文件时 serde 的「unknown field」。

/// 已经移除的字段：写了就明确告诉原因和替代写法，而不是猜一个相近的名字
const REMOVED: &[(&str, &str)] = &[
    ("exec", "已移除：requestbaby 不再运行用例里写的命令（打开别人的项目也不会在本机执行代码）；要通知请写 notify（requestbaby guide control）"),
    ("commands", "已移除：不再支持运行本机命令；默认通知地址写在 requestbaby.json 的 notify.webhook（requestbaby guide control）"),
];

/// 编辑距离（插入 / 删除 / 替换 / 相邻两个字母写反都算 1）
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

/// 候选里最像 `name` 的一个（大小写不同、少 / 多 / 错一两个字母）
pub fn closest<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let lower = name.to_lowercase();
    let limit = if name.chars().count() <= 4 { 1 } else { 2 };
    candidates
        .into_iter()
        .filter(|c| *c != name)
        .map(|c| (distance(&lower, &c.to_lowercase()), c))
        .filter(|(d, _)| *d <= limit)
        .min_by_key(|(d, c)| (*d, c.len()))
        .map(|(_, c)| c)
}

/// 一个不认识的字段怎么说
pub fn unknown_field<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> String {
    if let Some((_, why)) = REMOVED.iter().find(|(k, _)| *k == name) {
        return format!("字段 {name} {why}");
    }
    match closest(name, candidates) {
        Some(c) => format!("不认识的字段 {name}，是不是想写 {c}？"),
        None => format!("不认识的字段 {name}（requestbaby schema --all 可以看到全部字段，requestbaby guide ref 按用例类型列出）"),
    }
}

/// jsonschema 的「Additional properties are not allowed ('a', 'b' were unexpected)」→ 中文提示；不是这种错误返回 None
pub fn from_schema_message(msg: &str, known: &[String]) -> Option<String> {
    let rest = msg.strip_prefix("Additional properties are not allowed (")?;
    let names = rest.rsplit_once(" w")?.0;
    let parts: Vec<String> = names.split("', '").map(|n| n.trim_matches('\'').to_string()).filter(|n| !n.is_empty()).collect();
    if parts.is_empty() {
        return None;
    }
    Some(parts.iter().map(|n| unknown_field(n, known.iter().map(String::as_str))).collect::<Vec<_>>().join("；"))
}

/// serde 的「unknown field `x`, expected one of `a`, `b`」→ 中文提示（候选就在报错里，不再把整份字段列表倒出来）。其他错误原样返回
pub fn from_serde_message(msg: &str) -> String {
    let Some(i) = msg.find("unknown field `") else { return msg.to_string() };
    let after = &msg[i + "unknown field `".len()..];
    let Some((name, tail)) = after.split_once('`') else { return msg.to_string() };
    let candidates: Vec<&str> = tail.split('`').skip(1).step_by(2).collect();
    let at = tail
        .rsplit_once(" at line ")
        .and_then(|(_, pos)| pos.split_once(" column "))
        .map(|(l, c)| format!("（位置 第 {l} 行第 {c} 列）"))
        .unwrap_or_default();
    format!("{}{at}", unknown_field(name, candidates))
}

/// 内置 schema 里出现过的全部字段名（`properties` 的键），拼错字段时做候选
pub fn schema_keys(schema: &serde_json::Value) -> Vec<String> {
    fn walk(v: &serde_json::Value, out: &mut std::collections::BTreeSet<String>) {
        match v {
            serde_json::Value::Object(m) => {
                if let Some(serde_json::Value::Object(p)) = m.get("properties") {
                    out.extend(p.keys().cloned());
                }
                m.values().for_each(|x| walk(x, out));
            }
            serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut set = std::collections::BTreeSet::new();
    walk(schema, &mut set);
    set.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_the_nearest_field_and_explains_removed_ones() {
        let known: Vec<String> = ["expect", "request", "capture", "tags", "name", "timeout"].map(String::from).to_vec();
        let k = || known.iter().map(String::as_str);
        assert_eq!(unknown_field("expct", k()), "不认识的字段 expct，是不是想写 expect？");
        assert!(unknown_field("Expect", k()).contains("想写 expect"));
        assert!(unknown_field("zzzzzz", k()).contains("guide ref"));
        // 短名字只允许错一个字母，避免 tag → name 这类乱猜
        assert!(unknown_field("tag", k()).contains("想写 tags"));
        assert!(!unknown_field("nom", k()).contains("想写 name"));
        assert!(unknown_field("exec", k()).contains("notify"));
        let m = from_schema_message("Additional properties are not allowed ('expct' was unexpected)", &known).unwrap();
        assert!(m.contains("想写 expect"), "{m}");
        let m = from_schema_message("Additional properties are not allowed ('exec', 'tgas' were unexpected)", &known).unwrap();
        assert!(m.contains("notify") && m.contains("想写 tags"), "{m}");
        assert!(from_schema_message("\"x\" is not of type \"string\"", &known).is_none());
    }

    #[test]
    fn serde_messages_keep_the_position_but_drop_the_field_dump() {
        let m = from_serde_message("unknown field `expct`, expected one of `name`, `tags`, `expect`, `capture` at line 1 column 104");
        assert!(m.contains("想写 expect") && m.contains("第 1 行第 104 列") && !m.contains("`tags`"), "{m}");
        assert_eq!(from_serde_message("EOF while parsing"), "EOF while parsing");
    }
}
