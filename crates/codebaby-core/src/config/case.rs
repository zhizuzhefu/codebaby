use crate::vars::VarVal;
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
    /// 旧写法：期望状态码 200 或 [200,409]（新用例写 expect.status；`fmt --fix` 自动迁移）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect_status: Option<ExpectStatus>,
    /// HTTP 请求（与 grpc / ws 三选一）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<RequestSpec>,
    /// gRPC 调用（与 request / ws 三选一）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grpc: Option<super::protocols::GrpcSpec>,
    /// WebSocket 对话（与 request / grpc 三选一）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ws: Option<super::protocols::WsSpec>,
    /// 浏览器界面验收（Chrome DevTools 协议驱动本机浏览器）
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub regions: std::collections::BTreeMap<String, super::ui::Region>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web: Option<super::ui::WebSpec>,
    /// 安卓应用界面验收（adb + uiautomator）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub android: Option<super::ui::AndroidSpec>,
    /// Mac 桌面应用界面验收（agent-desktop 无障碍树）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desktop: Option<super::ui::DesktopSpec>,
    /// 前置脚本：请求发出前执行，可改写 req
    #[serde(default)]
    pub pre: String,
    /// 后置脚本：响应返回后、断言前执行，可写 vars 捕获
    #[serde(default)]
    pub post: String,
    /// 声明式断言（推荐）：status / headers / body 路径 → 期望，见 [`crate::expect`]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<crate::expect::Expect>,
    /// 声明式捕获：变量名 → 响应体路径（如 data.token）或 header:名称；先于 post 脚本执行
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub capture: serde_json::Map<String, serde_json::Value>,
    /// 进阶断言：JS 表达式数组，逐条求值（expect 表达不了的逻辑再用）
    #[serde(default)]
    pub tests: Vec<String>,
    /// 数据驱动：内联行数组（每行一个变量 map）或 {"file": "data.csv|.json"}
    /// 用例 × 每行数据 = 一个实例，行变量优先级最高
    #[serde(default)]
    pub cases: Option<CasesSpec>,
}

impl TestCase {
    /// 运行时写入的变量：capture 的键（含 ws 步骤里的）+ pre / post 脚本中的 `vars.x = …`（依赖分析、未定义变量检查共用）
    pub fn var_writes(&self) -> Vec<String> {
        let mut out: Vec<String> = self.capture.keys().cloned().collect();
        let ui_steps = self
            .web
            .as_ref()
            .map(|w| w.steps.as_slice())
            .or(self.android.as_ref().map(|a| a.steps.as_slice()))
            .or(self.desktop.as_ref().map(|d| d.steps.as_slice()))
            .unwrap_or_default();
        let ui_captures = ui_steps.iter().flat_map(|s| s.capture.keys().cloned());
        for n in self.ws.as_ref().map(|w| w.captures()).unwrap_or_default().into_iter().chain(ui_captures) {
            if !out.contains(&n) {
                out.push(n);
            }
        }
        for n in crate::validate::scan_var_writes(&self.pre).into_iter().chain(crate::validate::scan_var_writes(&self.post)) {
            if !out.contains(&n) {
                out.push(n);
            }
        }
        out
    }

    /// 协议：由写了 request / grpc / ws 中的哪一个决定（都没写按 HTTP）
    pub fn protocol(&self) -> super::protocols::Protocol {
        use super::protocols::Protocol;
        if self.grpc.is_some() {
            Protocol::Grpc
        } else if self.ws.is_some() {
            Protocol::Ws
        } else if self.web.is_some() {
            Protocol::Web
        } else if self.android.is_some() {
            Protocol::Android
        } else if self.desktop.is_some() {
            Protocol::Desktop
        } else {
            Protocol::Http
        }
    }

    /// 列表 / 报告里的一行摘要：(方法, 地址)。HTTP = (GET, url)；gRPC = (GRPC, 地址/包.服务/方法)；WebSocket = (WS, url)
    pub fn summary(&self) -> (String, String) {
        if let Some(g) = &self.grpc {
            ("GRPC".into(), format!("{}/{}", g.target.trim_end_matches('/'), g.method))
        } else if let Some(w) = &self.ws {
            ("WS".into(), w.url.clone())
        } else if let Some(w) = &self.web {
            ("WEB".into(), w.url.clone())
        } else if let Some(a) = &self.android {
            ("APP".into(), a.app.clone())
        } else if let Some(d) = &self.desktop {
            ("MAC".into(), d.app.clone())
        } else if let Some(r) = &self.request {
            (r.method.clone(), r.url.clone())
        } else {
            ("-".into(), String::new())
        }
    }

    /// request / grpc / ws / web / android / desktop 必须恰好写一个
    pub fn spec_error(&self) -> Option<String> {
        let n = [self.request.is_some(), self.grpc.is_some(), self.ws.is_some(), self.web.is_some(), self.android.is_some(), self.desktop.is_some()].iter().filter(|x| **x).count();
        match n {
            1 => None,
            0 => Some("缺少请求：写 request（HTTP）、grpc、ws、web、android 或 desktop 其中一个".into()),
            _ => Some("request / grpc / ws / web / android / desktop 只能写一个：一个用例对应一种协议".into()),
        }
    }

    /// 请求部分可能含 {{var}} 的文本
    pub fn request_texts(&self) -> Vec<String> {
        let mut texts = vec![];
        if let Some(r) = &self.request {
            texts.push(r.url.clone());
            texts.extend(r.headers.values().map(|v| v.value().to_string()));
            texts.extend(r.query.iter().flat_map(|(k, v)| [k.clone(), v.value().to_string()]));
            if let Some(b) = &r.body {
                texts.extend(b.texts());
            }
        }
        if let Some(g) = &self.grpc {
            texts.extend(g.texts());
        }
        if let Some(w) = &self.ws {
            texts.extend(w.texts());
        }
        if let Some(w) = &self.web {
            texts.push(w.url.clone());
            texts.extend(w.steps.iter().flat_map(|s| s.texts()));
        }
        if let Some(a) = &self.android {
            texts.extend([a.app.clone()].into_iter().chain(a.activity.clone()).chain(a.apk.clone()));
            texts.extend(a.steps.iter().flat_map(|s| s.texts()));
        }
        if let Some(d) = &self.desktop {
            texts.extend([d.app.clone()].into_iter().chain(d.window.clone()));
            texts.extend(d.steps.iter().flat_map(|s| s.texts()));
        }
        texts
    }

    /// 可能引用变量的全部文本：请求（url / headers / query / body / 消息 / 步骤）、脚本、JS 断言、expect 期望值
    pub fn referencing_texts(&self) -> Vec<String> {
        let mut texts = vec![self.pre.clone(), self.post.clone()];
        texts.extend(self.request_texts());
        texts.extend(self.tests.iter().cloned());
        if let Some(e) = &self.expect {
            texts.extend(crate::expect::referenced_texts(e));
        }
        texts
    }

    /// 期望状态码：expect.status 优先，其次旧写法 expectStatus
    pub fn expected_status(&self) -> Option<&ExpectStatus> {
        self.expect.as_ref().and_then(|e| e.status.as_ref()).or(self.expect_status.as_ref())
    }
}

/// 数据驱动来源
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum CasesSpec {
    /// 内联：[{"label":"边界","qty":0,"vip":true,"sku":"A-1"}, ...]（值保留 JSON 原生类型）
    Inline(Vec<BTreeMap<String, serde_json::Value>>),
    /// 外部文件：{"file": "./data/users.csv"}（CSV 首行表头，值为字符串）或 JSON 对象数组文件（保留原生类型）
    File { file: String },
}

/// 一行数据：变量名 → 值（数字 / 布尔保留原生类型，脚本里 `env.qty === 0` 直接成立）
pub type DataRow = BTreeMap<String, VarVal>;

impl CasesSpec {
    /// 解析为行列表（file 相对用例文件目录）
    pub fn resolve(&self, base_dir: &std::path::Path) -> std::io::Result<Vec<DataRow>> {
        match self {
            CasesSpec::Inline(rows) => Ok(rows.iter().map(json_row).collect()),
            CasesSpec::File { file } => {
                let content = std::fs::read_to_string(base_dir.join(file))?;
                if file.ends_with(".csv") {
                    return Ok(parse_csv(&content)
                        .into_iter()
                        .map(|r| r.into_iter().map(|(k, v)| (k, VarVal::Str(v))).collect())
                        .collect());
                }
                let v: serde_json::Value = serde_json::from_str(&content)
                    .map_err(|e| std::io::Error::other(format!("JSON 数据文件解析失败: {e}")))?;
                let arr = v.as_array().ok_or_else(|| std::io::Error::other("JSON 数据文件必须是对象数组"))?;
                arr.iter()
                    .map(|row| {
                        row.as_object()
                            .map(|o| json_row(&o.iter().map(|(k, x)| (k.clone(), x.clone())).collect()))
                            .ok_or_else(|| std::io::Error::other("JSON 数据文件的每一项必须是对象"))
                    })
                    .collect()
            }
        }
    }
}

fn json_row(row: &BTreeMap<String, serde_json::Value>) -> DataRow {
    row.iter().map(|(k, v)| (k.clone(), json_scalar(v))).collect()
}

/// JSON 标量 → 变量值。超出 f64 精确范围的整数（雪花 ID 等）保留原文字符串，避免精度丢失
fn json_scalar(v: &serde_json::Value) -> VarVal {
    match v {
        serde_json::Value::String(s) => VarVal::Str(s.clone()),
        serde_json::Value::Bool(b) => VarVal::Bool(*b),
        serde_json::Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) if i.unsigned_abs() > (1u64 << 53) => VarVal::Str(n.to_string()),
            (None, _) if n.as_u64().is_some() => VarVal::Str(n.to_string()),
            (_, Some(f)) => VarVal::Num(f),
            _ => VarVal::Str(n.to_string()),
        },
        serde_json::Value::Null => VarVal::Str(String::new()),
        other => VarVal::Str(other.to_string()),
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

/// 期望状态码：单值、列表，或数据驱动行里的变量 `"{{status}}"`；
/// gRPC 另可写状态名 `"NOT_FOUND"` / `["OK", "NOT_FOUND"]`
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ExpectStatus {
    One(u16),
    Many(Vec<u16>),
    Var(String),
    Names(Vec<String>),
}

impl ExpectStatus {
    /// 可接受的 HTTP 状态码；变量形态代入后解析（`200` 或 `200,201`）
    pub fn resolve(&self, vars: &crate::vars::Vars) -> Result<Vec<u16>, String> {
        self.resolve_for(super::protocols::Protocol::Http, vars)
    }

    /// 按协议解析：HTTP / WebSocket 是 HTTP 状态码（100–599），gRPC 是状态码 0–16 或状态名
    pub fn resolve_for(&self, protocol: super::protocols::Protocol, vars: &crate::vars::Vars) -> Result<Vec<u16>, String> {
        use super::protocols::{parse_grpc_code, Protocol};
        let grpc = protocol == Protocol::Grpc;
        let one = |text: &str| -> Option<u16> {
            if grpc {
                parse_grpc_code(text)
            } else {
                text.trim().parse::<u16>().ok().filter(|c| (100..600).contains(c))
            }
        };
        let kind = if grpc { "gRPC 状态（OK、NOT_FOUND… 或 0–16）" } else { "状态码" };
        match self {
            ExpectStatus::One(c) if grpc && *c > 16 => Err(format!("{c} 不是{kind}")),
            ExpectStatus::One(c) => Ok(vec![*c]),
            ExpectStatus::Many(v) if grpc && v.iter().any(|c| *c > 16) => Err(format!("{v:?} 里有不是{kind}的值")),
            ExpectStatus::Many(v) => Ok(v.clone()),
            ExpectStatus::Names(names) => {
                let codes: Option<Vec<u16>> = names.iter().map(|n| one(n)).collect();
                codes.ok_or_else(|| format!("{names:?} 里有不是{kind}的值"))
            }
            ExpectStatus::Var(t) => {
                let text = vars.render(t);
                let codes: Option<Vec<u16>> = text.split(',').map(one).collect();
                codes.filter(|c| !c.is_empty()).ok_or_else(|| {
                    if text.contains("{{") {
                        format!("状态码变量 {t} 未定义")
                    } else if t.contains("{{") {
                        format!("状态码变量 {t} 的值不是{kind}：{text}")
                    } else {
                        format!("{text} 不是{kind}")
                    }
                })
            }
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RequestSpec {
    pub method: String,
    pub url: String,
    /// 请求头；值可写 `{"value": "...", "disabled": true}` 保留但不发送
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, KvValue>,
    /// 查询参数：按 URL 编码追加到 url 之后（url 自身已带的 ?a=1 保留）；同样支持 disabled
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub query: BTreeMap<String, KvValue>,
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

/// 键值表（headers / query）的值：字符串简写，或带启用开关的完整形态
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum KvValue {
    Plain(String),
    Full {
        value: String,
        /// true = 保留在用例里但不发送（调试时临时关掉某个头/参数）
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        disabled: bool,
    },
}

impl KvValue {
    pub fn value(&self) -> &str {
        match self {
            KvValue::Plain(v) | KvValue::Full { value: v, .. } => v,
        }
    }

    pub fn enabled(&self) -> bool {
        !matches!(self, KvValue::Full { disabled: true, .. })
    }

    /// 启用的写回简写字符串，停用的写成 {"value","disabled":true}
    pub fn new(value: String, enabled: bool) -> Self {
        if enabled {
            KvValue::Plain(value)
        } else {
            KvValue::Full { value, disabled: true }
        }
    }
}

impl From<&str> for KvValue {
    fn from(v: &str) -> Self {
        KvValue::Plain(v.to_string())
    }
}

/// 只保留启用的键值
pub fn enabled_kv(map: &BTreeMap<String, KvValue>) -> impl Iterator<Item = (&String, &str)> {
    map.iter().filter(|(_, v)| v.enabled()).map(|(k, v)| (k, v.value()))
}

/// 请求体形态：
/// - {"json": <对象/数组>}：序列化为 JSON，未设 Content-Type 时自动 application/json
/// - {"text": "<字符串>"}：原样发送
/// - {"form": {"k": "v"}}：application/x-www-form-urlencoded，逐字段 URL 编码
/// - {"multipart": {"k": "v", "file": {"file": "a.png"}}}：multipart/form-data，文件路径相对用例文件
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
    Form {
        form: BTreeMap<String, String>,
    },
    Multipart {
        multipart: BTreeMap<String, MultipartValue>,
    },
    Raw(String),
}

/// multipart 字段：文本，或文件
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum MultipartValue {
    Text(String),
    File {
        /// 文件路径：相对用例文件所在目录，或绝对路径；支持 {{var}}
        file: String,
        /// 上传时的文件名（缺省取路径文件名）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        /// 该部分的 Content-Type（缺省由扩展名推断）
        #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
    },
}

impl BodySpec {
    /// 展开为最终字符串（`render` 对每段文本做变量替换）；multipart 不走字符串 body，返回 None
    pub fn render_with(&self, render: impl Fn(&str) -> String) -> Option<String> {
        Some(match self {
            BodySpec::Json { json } => render(&serde_json::to_string(json).unwrap_or_default()),
            BodySpec::Text { text } => render(text),
            BodySpec::Raw(s) => render(s),
            BodySpec::Form { form } => form
                .iter()
                .map(|(k, v)| format!("{}={}", form_encode(&render(k)), form_encode(&render(v))))
                .collect::<Vec<_>>()
                .join("&"),
            BodySpec::Multipart { .. } => return None,
        })
    }

    /// 全部可能含 {{var}} 的文本（变量引用检查用）
    pub fn texts(&self) -> Vec<String> {
        match self {
            BodySpec::Form { form } => form.iter().flat_map(|(k, v)| [k.clone(), v.clone()]).collect(),
            BodySpec::Multipart { multipart } => multipart
                .iter()
                .flat_map(|(k, v)| match v {
                    MultipartValue::Text(t) => vec![k.clone(), t.clone()],
                    MultipartValue::File { file, filename, .. } => {
                        vec![k.clone(), file.clone(), filename.clone().unwrap_or_default()]
                    }
                })
                .collect(),
            other => other.render_with(str::to_string).into_iter().collect(),
        }
    }

    pub fn is_json(&self) -> bool {
        matches!(self, BodySpec::Json { .. })
    }

    /// 未显式设置时自动补的 Content-Type（multipart 由 libcurl 生成带 boundary 的头）
    pub fn default_content_type(&self) -> Option<&'static str> {
        match self {
            BodySpec::Json { .. } => Some("application/json"),
            BodySpec::Form { .. } => Some("application/x-www-form-urlencoded"),
            _ => None,
        }
    }
}

/// application/x-www-form-urlencoded / 查询参数编码（空格→%20）。
/// 未替换的 `{{var}}` 原样保留，残留检查仍能报出「变量未定义」而不是发出编码后的乱码
pub fn form_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while !rest.is_empty() {
        if rest.starts_with("{{") {
            if let Some(end) = rest.find("}}") {
                out.push_str(&rest[..end + 2]);
                rest = &rest[end + 2..];
                continue;
            }
        }
        let c = rest.chars().next().unwrap();
        if c.is_ascii_alphanumeric() || "-_.~".contains(c) {
            out.push(c);
        } else {
            let mut buf = [0u8; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        }
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// 把查询参数追加到 URL（保留 URL 已有的 query 与 #fragment）
pub fn append_query(url: &str, pairs: &[(String, String)]) -> String {
    if pairs.is_empty() {
        return url.to_string();
    }
    let (base, frag) = match url.find('#') {
        Some(i) => (&url[..i], &url[i..]),
        None => (url, ""),
    };
    let q = pairs.iter().map(|(k, v)| format!("{}={}", form_encode(k), form_encode(v))).collect::<Vec<_>>().join("&");
    let sep = if !base.contains('?') {
        "?"
    } else if base.ends_with('?') || base.ends_with('&') {
        ""
    } else {
        "&"
    };
    format!("{base}{sep}{q}{frag}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kv_values_and_query() {
        let r: RequestSpec = serde_json::from_str(
            r#"{"method":"GET","url":"{{base}}/s?x=1#top","headers":{"A":"1","B":{"value":"2","disabled":true}},"query":{"q":"红 茶&","page":"{{p}}"}}"#,
        )
        .unwrap();
        let on: Vec<_> = enabled_kv(&r.headers).collect();
        assert_eq!(on, vec![(&"A".to_string(), "1")]);
        // 写回：启用的保持字符串简写，停用的保持对象形态
        let back = serde_json::to_value(&r).unwrap();
        assert_eq!(back["headers"]["A"], "1");
        assert_eq!(back["headers"]["B"]["disabled"], true);
        let pairs: Vec<(String, String)> = enabled_kv(&r.query).map(|(k, v)| (k.clone(), v.to_string())).collect();
        assert_eq!(append_query(&r.url, &pairs), "{{base}}/s?x=1&page={{p}}&q=%E7%BA%A2%20%E8%8C%B6%26#top");
        assert_eq!(append_query("http://h/a", &pairs[..1]), "http://h/a?page={{p}}");
    }

    #[test]
    fn form_and_multipart_bodies() {
        let b: BodySpec = serde_json::from_str(r#"{"form":{"user":"{{u}}","note":"a b&c"}}"#).unwrap();
        assert_eq!(b.render_with(|s| s.replace("{{u}}", "张三")).unwrap(), "note=a%20b%26c&user=%E5%BC%A0%E4%B8%89");
        assert_eq!(b.default_content_type(), Some("application/x-www-form-urlencoded"));
        let m: BodySpec =
            serde_json::from_str(r#"{"multipart":{"title":"t","doc":{"file":"fixtures/a.pdf","type":"application/pdf"}}}"#).unwrap();
        let BodySpec::Multipart { multipart } = &m else { panic!("{m:?}") };
        assert!(matches!(&multipart["doc"], MultipartValue::File { file, content_type: Some(t), .. } if file == "fixtures/a.pdf" && t == "application/pdf"));
        assert!(m.render_with(str::to_string).is_none());
        assert!(m.texts().contains(&"fixtures/a.pdf".to_string()));
    }

    #[test]
    fn inline_rows_keep_json_types() {
        let spec: CasesSpec = serde_json::from_str(
            r#"[{"label":"边界","qty":0,"price":9.5,"vip":true,"sku":"A-1","snowflake":1234567890123456789,"none":null}]"#,
        )
        .unwrap();
        let rows = spec.resolve(std::path::Path::new(".")).unwrap();
        let r = &rows[0];
        assert_eq!(r["qty"], VarVal::Num(0.0));
        assert_eq!(r["price"], VarVal::Num(9.5));
        assert_eq!(r["vip"], VarVal::Bool(true));
        assert_eq!(r["sku"], VarVal::Str("A-1".into()));
        // 超出 f64 精确范围：保留原文，避免 1234567890123456800 这类精度丢失
        assert_eq!(r["snowflake"], VarVal::Str("1234567890123456789".into()));
        assert_eq!(r["none"], VarVal::Str(String::new()));
    }

    #[test]
    fn json_file_rows_keep_types_csv_rows_are_strings() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("d.json"), r#"[{"n":2,"ok":false}]"#).unwrap();
        std::fs::write(dir.path().join("d.csv"), "n,ok\n2,false\n").unwrap();
        let json = CasesSpec::File { file: "d.json".into() }.resolve(dir.path()).unwrap();
        assert_eq!(json[0]["n"], VarVal::Num(2.0));
        assert_eq!(json[0]["ok"], VarVal::Bool(false));
        let csv = CasesSpec::File { file: "d.csv".into() }.resolve(dir.path()).unwrap();
        assert_eq!(csv[0]["n"], VarVal::Str("2".into()));
    }
}
