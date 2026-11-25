use super::StringVars;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 项目根配置 requestbaby.json
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Project {
    /// 配置格式版本（当前 1）
    #[serde(default = "default_version")]
    pub version: u32,
    /// 默认环境名（未指定 -e 时使用）
    #[serde(default = "default_env")]
    pub default_env: String,
    /// 全局变量（最低优先级）
    #[serde(default)]
    pub vars: StringVars,
    /// 默认请求超时（毫秒）
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    /// 命名流程：多阶段编排（数据参数经 runtime vars 跨阶段自动传递）
    #[serde(default)]
    pub workflows: BTreeMap<String, Workflow>,
    /// 自动管理 Cookie：保存响应的 Set-Cookie，后续同域请求自动携带（缺省开启）
    #[serde(default = "default_true")]
    pub cookies: bool,
    /// gRPC：.proto 文件来源（不配置则使用服务端反射）
    #[serde(default, skip_serializing_if = "GrpcConfig::is_empty")]
    pub grpc: GrpcConfig,
    /// Web 界面验收：浏览器、视口、截图策略
    #[serde(default, skip_serializing_if = "is_default")]
    pub web: super::ui::WebConfig,
    /// Android 界面验收：adb、设备、截图策略
    #[serde(default, skip_serializing_if = "is_default")]
    pub android: super::ui::AndroidConfig,
    /// Mac 桌面应用验收：agent-desktop、截图策略
    #[serde(default, skip_serializing_if = "is_default")]
    pub desktop: super::ui::DesktopConfig,
    /// 屏幕文字识别（OCR）：识别语言
    #[serde(default, skip_serializing_if = "is_default")]
    pub ocr: super::ui::OcrConfig,
    /// 通知：全项目默认的 webhook，步骤里的 `notify` 不写 webhook 时发到这里
    #[serde(default, skip_serializing_if = "is_default")]
    pub notify: NotifyConfig,
    /// 代理：所有对外连接（HTTP / gRPC / WebSocket / 浏览器 / adb）经同一个代理；不配置则跟随系统（环境变量 → 系统代理设置）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<ProxyConfig>,
}

/// 通知设置（requestbaby.json 的 `notify`）
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NotifyConfig {
    /// 默认 webhook：通知内容以 `{"text": "…"}` POST 到这个地址（飞书 / 钉钉 / 企业微信 / Slack 的机器人地址换一下格式即可）；可写 {{变量}}（如密钥放环境变量）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook: Option<String>,
}

/// 代理设置（requestbaby.json 的 `proxy`）
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProxyConfig {
    /// 是否使用代理：false = 全部直连（也不读 http_proxy 等环境变量），保留 url 方便再打开
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 代理地址：socks5://主机:端口 / socks5h://（域名由代理解析）/ http://；可带 user:pass@，可写 {{变量}}
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// 不走代理的主机：example.com（含子域）/ *.corp / 10.0.0.0/8；本机地址始终直连
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub no_proxy: Vec<String>,
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// gRPC 的接口描述来源
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct GrpcConfig {
    /// .proto 文件或目录（目录下全部 .proto），相对项目根；为空 = 向服务端反射查询
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub protos: Vec<String>,
    /// import 搜索目录（相对项目根）；缺省 = protos 里的目录 + 各文件所在目录
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub import_paths: Vec<String>,
}

impl GrpcConfig {
    pub fn is_empty(&self) -> bool {
        self.protos.is_empty() && self.import_paths.is_empty()
    }
}

/// 一个流程 = 有序步骤列表
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Workflow {
    /// 流程显示名（缺省用 key）
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// 固定运行环境（如本地 fixture 只能跑 local）：缺省跟随 -e / defaultEnv；
    /// 设置后 `verify` 在该环境运行它，显式 -e 其他环境会被拒绝；verify 在其他环境时跳过它
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    /// 依赖的流程（如「登录」）：运行本流程前先运行它们，同一次运行里每个依赖只跑一次，
    /// 它们留下的登录态（Cookie、网页存储、捕获的变量）本流程的所有步骤直接沿用
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<String>,
    /// 有序步骤（只有依赖、没有自己的步骤也可以）
    #[serde(default)]
    pub steps: Vec<WorkflowStep>,
}

/// 单个步骤 = 一次 run（路径 + 过滤），变量与前序步骤自动共享
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkflowStep {
    /// 步骤名（缺省用 run 路径）
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// 运行目标（文件/目录）
    pub run: String,
    /// 标签过滤
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// 用例名过滤
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name_filter: String,
    /// 以哪个用户运行（如 teacher / student）：每个用户有各自独立的登录态（Cookie、网页存储），
    /// 切换用户 = 换一个名字；捕获的变量仍然共享。缺省 = 默认用户
    #[serde(default, rename = "as", skip_serializing_if = "Option::is_none")]
    pub as_user: Option<String>,
}

fn default_version() -> u32 {
    1
}
fn default_env() -> String {
    "dev".to_string()
}
fn default_timeout() -> u64 {
    30_000
}
fn default_true() -> bool {
    true
}

impl Default for Project {
    fn default() -> Self {
        Self {
            version: default_version(),
            default_env: default_env(),
            vars: StringVars::default(),
            timeout_ms: default_timeout(),
            workflows: BTreeMap::new(),
            cookies: true,
            grpc: GrpcConfig::default(),
            web: Default::default(),
            android: Default::default(),
            desktop: Default::default(),
            ocr: Default::default(),
            notify: NotifyConfig::default(),
            proxy: None,
        }
    }
}
