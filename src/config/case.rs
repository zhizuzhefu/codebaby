use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 单个测试用例（一个 JSON 文件）
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TestCase {
    /// 用例名（缺省用文件名去扩展名）
    #[serde(default)]
    pub name: String,
    /// 标签（与集合级标签合并，run --tag 过滤）
    #[serde(default)]
    pub tags: Vec<String>,
    /// 跳过该用例（计划保留但暂不执行）
    #[serde(default)]
    pub skip: bool,
    /// 期望状态码语法糖：200 或 [200,409]，等价于隐式首条断言 [200,409].includes(res.status)
    #[serde(default)]
    pub expect_status: Option<ExpectStatus>,
    pub request: RequestSpec,
    /// 前置脚本：请求发出前执行，可改写 req
    #[serde(default)]
    pub pre: String,
    /// 后置脚本：响应返回后、断言前执行，可写 vars 捕获
    #[serde(default)]
    pub post: String,
    /// 断言：JS 表达式数组，逐条求值
    #[serde(default)]
    pub tests: Vec<String>,
    /// 数据驱动：内联行数组（每行一个变量 map）或 {"file": "data.csv|.json"}
    /// 用例 × 每行数据 = 一个实例，行变量优先级最高
    #[serde(default)]
    pub cases: Option<CasesSpec>,
}

/// 数据驱动来源
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum CasesSpec {
    /// 内联：[{"name":"alice","age":"18"}, ...]
    Inline(Vec<BTreeMap<String, String>>),
    /// 外部文件：{"file": "./data/users.csv"}（CSV 首行表头）或 JSON 数组文件
    File { file: String },
}

impl CasesSpec {
    /// 解析为行列表（file 相对用例文件目录）
    pub fn resolve(&self, base_dir: &std::path::Path) -> std::io::Result<Vec<BTreeMap<String, String>>> {
        match self {
            CasesSpec::Inline(rows) => Ok(rows.clone()),
            CasesSpec::File { file } => {
                let path = base_dir.join(file);
                let content = std::fs::read_to_string(path)?;
                if file.ends_with(".csv") {
                    Ok(parse_csv(&content))
                } else {
                    let v: serde_json::Value = serde_json::from_str(&content)
                        .map_err(|e| std::io::Error::other(format!("JSON 数据文件解析失败: {e}")))?;
                    let arr = v
                        .as_array()
                        .ok_or_else(|| std::io::Error::other("JSON 数据文件必须是对象数组"))?;
                    Ok(arr
                        .iter()
                        .map(|row| {
                            row.as_object()
                                .map(|o| {
                                    o.iter()
                                        .map(|(k, x)| {
                                            (k.clone(), match x {
                                                serde_json::Value::String(s) => s.clone(),
                                                serde_json::Value::Null => String::new(),
                                                other => other.to_string(),
                                            })
                                        })
                                        .collect()
                                })
                                .unwrap_or_default()
                        })
                        .collect())
                }
            }
        }
    }
}

/// 简易 CSV：首行表头，支持字段双引号转义
fn parse_csv(content: &str) -> Vec<BTreeMap<String, String>> {
    let mut lines = content.lines().filter(|l| !l.trim().is_empty());
    let Some(header) = lines.next() else { return vec![] };
    let header: Vec<String> = header.split(',').map(|s| s.trim().to_string()).collect();
    let mut rows = vec![];
    for line in lines {
        let fields = split_csv_line(line);
        let mut m = BTreeMap::new();
        for (i, h) in header.iter().enumerate() {
            m.insert(h.clone(), fields.get(i).cloned().unwrap_or_default());
        }
        rows.push(m);
    }
    rows
}

fn split_csv_line(line: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut in_quote = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                if in_quote && chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    in_quote = !in_quote;
                }
            }
            ',' if !in_quote => {
                out.push(cur.clone());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// expectStatus: 单值或列表
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ExpectStatus {
    One(u16),
    Many(Vec<u16>),
}

impl ExpectStatus {
    pub fn to_list(&self) -> Vec<u16> {
        match self {
            ExpectStatus::One(c) => vec![*c],
            ExpectStatus::Many(v) => v.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RequestSpec {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: Option<BodySpec>,
    /// 单请求超时毫秒（缺省用项目配置）
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// 传输层重试次数
    #[serde(default)]
    pub retry: Option<u32>,
    /// 跳过 TLS 证书校验
    #[serde(default)]
    pub insecure: Option<bool>,
    /// 跟随 3xx 重定向
    #[serde(default)]
    pub follow_redirects: Option<bool>,
}

/// 请求体形态：
/// - {"json": <对象/数组>}：序列化为 JSON，未设 Content-Type 时自动 application/json
/// - {"text": "<字符串>"}：原样发送
/// - "<字符串>"：等价 text（裸字符串简写）
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum BodySpec {
    Json {
        json: serde_json::Value,
    },
    Text {
        text: String,
    },
    Raw(String),
}

impl BodySpec {
    /// 展开为最终字符串
    pub fn render(&self) -> String {
        match self {
            BodySpec::Json { json } => serde_json::to_string(json).unwrap_or_default(),
            BodySpec::Text { text } => text.clone(),
            BodySpec::Raw(s) => s.clone(),
        }
    }

    pub fn is_json(&self) -> bool {
        matches!(self, BodySpec::Json { .. })
    }
}
