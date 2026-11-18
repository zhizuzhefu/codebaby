//! gRPC 引擎：tonic 传输 + prost-reflect 动态消息，不需要为被测服务生成代码。
//!
//! 接口描述两种来源：
//! - 项目配置了 `grpc.protos`：用 protox（纯 Rust 的 protobuf 编译器）现场编译 .proto
//! - 否则向服务端反射查询（grpc.reflection.v1，回落 v1alpha）
//!
//! 一元 / 服务端流 / 客户端流 / 双向流四种调用都支持：请求消息按顺序发出（流式时发完即结束发送），
//! 响应消息全部收齐；流式响应的 body 是消息数组。
use super::tls;
use anyhow::{anyhow, bail, Context, Result};
use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage, Kind, MessageDescriptor, MethodDescriptor, SerializeOptions};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tonic::codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};
use tonic::metadata::{KeyAndValueRef, MetadataMap};
use tonic::transport::{Channel, Endpoint};

/// 接口描述来源
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// .proto 文件（绝对路径）与 import 搜索目录
    Protos { files: Vec<PathBuf>, includes: Vec<PathBuf> },
    /// 服务端反射
    Reflection,
}

impl Source {
    /// 按项目配置展开：目录 → 其下全部 .proto；include 缺省 = 列出的目录 + 文件所在目录 + 项目根
    pub fn from_config(root: &Path, cfg: &crate::config::GrpcConfig) -> Result<Source> {
        if cfg.protos.is_empty() {
            return Ok(Source::Reflection);
        }
        let mut files = vec![];
        let mut includes: Vec<PathBuf> = cfg.import_paths.iter().map(|p| root.join(p)).collect();
        for p in &cfg.protos {
            let path = root.join(p);
            if path.is_dir() {
                push_unique(&mut includes, path.clone());
                for e in walkdir::WalkDir::new(&path).sort_by_file_name().into_iter().flatten() {
                    if e.file_type().is_file() && e.path().extension().is_some_and(|x| x == "proto") {
                        files.push(e.path().to_path_buf());
                    }
                }
            } else if path.is_file() {
                if let Some(parent) = path.parent() {
                    push_unique(&mut includes, parent.to_path_buf());
                }
                files.push(path);
            } else {
                bail!("grpc.protos 里的 {p} 不存在（相对项目根）");
            }
        }
        push_unique(&mut includes, root.to_path_buf());
        if files.is_empty() {
            bail!("grpc.protos 指向的目录里没有 .proto 文件");
        }
        Ok(Source::Protos { files, includes })
    }

    pub fn describe(&self) -> String {
        match self {
            Source::Reflection => "服务端反射".into(),
            Source::Protos { files, .. } => format!("{} 个 .proto 文件", files.len()),
        }
    }
}

fn push_unique(v: &mut Vec<PathBuf>, p: PathBuf) {
    if !v.contains(&p) {
        v.push(p);
    }
}

/// 一次调用（变量已替换）
#[derive(Debug, Clone)]
pub struct GrpcCall {
    pub target: String,
    /// `包.服务/方法`
    pub method: String,
    /// 请求消息（JSON 文本），按顺序发送
    pub messages: Vec<String>,
    pub metadata: Vec<(String, String)>,
    pub timeout_ms: u64,
    pub insecure: bool,
    pub source: Source,
    pub proxy: super::proxy::ProxyPolicy,
}

/// 调用结果（拿到 gRPC 状态即算有响应；连不上 / 超时等返回 Err）
#[derive(Debug, Clone, Default)]
pub struct GrpcReply {
    /// gRPC 状态码（0 = OK）
    pub code: u16,
    pub status_message: String,
    /// 响应元数据：headers + trailers（-bin 结尾的值为 base64），另含 grpc-status / grpc-message
    pub metadata: Vec<(String, String)>,
    /// 收到的响应消息（protobuf JSON；字段名同 .proto，默认值也输出）
    pub messages: Vec<Value>,
    /// 方法是服务端流 / 双向流（body 为消息数组）
    pub server_streaming: bool,
    /// 实际发出的请求消息（规范化后的 JSON）
    pub sent: Vec<Value>,
    pub time_ms: u64,
    pub size: u64,
}

impl GrpcReply {
    /// 断言 / 捕获 / 脚本看到的响应体：一元 = 单条消息；流式 = 消息数组
    pub fn body(&self) -> Value {
        if self.server_streaming {
            Value::Array(self.messages.clone())
        } else {
            self.messages.first().cloned().unwrap_or(Value::Null)
        }
    }
}

/// 方法信息（列表 / 描述 / 新建用例用）
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MethodInfo {
    /// `包.服务/方法`
    pub method: String,
    pub client_streaming: bool,
    pub server_streaming: bool,
    pub input_type: String,
    pub output_type: String,
    /// 请求消息骨架（全部字段的默认值）
    pub template: Value,
}

impl MethodInfo {
    fn of(m: &MethodDescriptor) -> Self {
        Self {
            method: format!("{}/{}", m.parent_service().full_name(), m.name()),
            client_streaming: m.is_client_streaming(),
            server_streaming: m.is_server_streaming(),
            input_type: m.input().full_name().to_string(),
            output_type: m.output().full_name().to_string(),
            template: template(&m.input(), 0),
        }
    }

    /// 一元 / 服务端流 / 客户端流 / 双向流
    pub fn kind_label(&self) -> &'static str {
        kind_label(self.client_streaming, self.server_streaming)
    }
}

pub fn kind_label(client: bool, server: bool) -> &'static str {
    match (client, server) {
        (false, false) => "一元",
        (false, true) => "服务端流",
        (true, false) => "客户端流",
        (true, true) => "双向流",
    }
}

// ---------------------------------------------------------------- 地址

/// 解析后的地址
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub tls: bool,
    /// host:port
    pub authority: String,
}

impl Target {
    pub fn host(&self) -> &str {
        match self.authority.rsplit_once(':') {
            Some((h, p)) if p.parse::<u16>().is_ok() => h.trim_start_matches('[').trim_end_matches(']'),
            _ => &self.authority,
        }
    }

    fn uri(&self) -> String {
        format!("{}://{}", if self.tls { "https" } else { "http" }, self.authority)
    }
}

/// `host:port` / `grpc://` / `http://` = 明文；`grpcs://` / `https://` = TLS（缺省端口 443）
pub fn parse_target(raw: &str) -> Result<Target> {
    let t = raw.trim();
    if t.is_empty() {
        bail!("gRPC 地址为空：写 host:port，TLS 写 grpcs://host:port");
    }
    let (tls, rest) = match t.split_once("://") {
        Some((s, r)) => match s.to_ascii_lowercase().as_str() {
            "grpc" | "http" | "h2c" => (false, r),
            "grpcs" | "https" => (true, r),
            other => bail!("gRPC 地址的协议 {other}:// 不支持：明文写 host:port 或 grpc://，TLS 写 grpcs://"),
        },
        None => (false, t),
    };
    let host_port = rest.split('/').next().unwrap_or_default().trim();
    if host_port.is_empty() {
        bail!("gRPC 地址 {raw} 缺少主机");
    }
    let has_port = host_port.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().is_ok()) && !host_port.ends_with(']');
    let authority = if has_port {
        host_port.to_string()
    } else if tls {
        format!("{host_port}:443")
    } else {
        bail!("gRPC 地址 {raw} 缺少端口：写 host:port（TLS 写 grpcs://host，缺省 443）")
    };
    Ok(Target { tls, authority })
}

/// `包.服务/方法`（也接受 `包.服务.方法`）→ (服务全名, 方法名)
pub fn split_method(full: &str) -> Result<(String, String)> {
    let f = full.trim().trim_start_matches('/');
    let (svc, m) = f.rsplit_once('/').or_else(|| f.rsplit_once('.')).ok_or_else(|| anyhow!("方法名 {full} 应写成 包.服务/方法"))?;
    if svc.is_empty() || m.is_empty() {
        bail!("方法名 {full} 应写成 包.服务/方法");
    }
    Ok((svc.to_string(), m.to_string()))
}

// ---------------------------------------------------------------- 公开入口（同步）

/// 执行一次调用
pub fn call(c: &GrpcCall) -> Result<GrpcReply> {
    let target = parse_target(&c.target)?;
    let (svc, method) = split_method(&c.method)?;
    let rt = shared_runtime()?;
    let timeout = Duration::from_millis(c.timeout_ms.max(1));
    let started = Instant::now();
    rt.block_on(async {
        tokio::time::timeout(timeout, async {
            let channel = channel(&target, c.insecure, &c.proxy, timeout).await?;
            let md = resolve_method(&channel, &target, &c.source, &svc, &method).await?;
            invoke(channel, &md, c, timeout, started).await
        })
        .await
        .map_err(|_| anyhow!("{} ms 内没有完成：服务太慢或卡住了（需要时调大 timeoutMs）", c.timeout_ms))?
    })
}

/// 列出全部方法（反射：服务端上的全部服务；proto：文件里定义的全部服务）
pub fn list_methods(target: &str, insecure: bool, timeout_ms: u64, source: &Source, proxy: &super::proxy::ProxyPolicy) -> Result<Vec<MethodInfo>> {
    let pool = match source {
        Source::Protos { .. } => proto_pool(source)?,
        Source::Reflection => {
            let target = parse_target(target)?;
            let rt = shared_runtime()?;
            let timeout = Duration::from_millis(timeout_ms.max(1));
            rt.block_on(async {
                tokio::time::timeout(timeout, async {
                    let channel = channel(&target, insecure, proxy, timeout).await?;
                    reflect_all(&channel, &target).await
                })
                .await
                .map_err(|_| anyhow!("{timeout_ms} ms 内没有完成反射查询"))?
            })?
        }
    };
    let mut out: Vec<MethodInfo> = pool
        .services()
        .filter(|s| !s.full_name().starts_with("grpc.reflection.") && !s.full_name().starts_with("grpc.health."))
        .flat_map(|s| s.methods().map(|m| MethodInfo::of(&m)).collect::<Vec<_>>())
        .collect();
    out.sort_by(|a, b| a.method.cmp(&b.method));
    Ok(out)
}

/// 单个方法的信息与请求骨架
pub fn describe_method(target: &str, method: &str, insecure: bool, timeout_ms: u64, source: &Source, proxy: &super::proxy::ProxyPolicy) -> Result<MethodInfo> {
    let (svc, name) = split_method(method)?;
    let md = match source {
        Source::Protos { .. } => find_method(&proto_pool(source)?, &svc, &name, source)?,
        Source::Reflection => {
            let t = parse_target(target)?;
            let rt = shared_runtime()?;
            let timeout = Duration::from_millis(timeout_ms.max(1));
            rt.block_on(async {
                tokio::time::timeout(timeout, async {
                    let channel = channel(&t, insecure, proxy, timeout).await?;
                    resolve_method(&channel, &t, source, &svc, &name).await
                })
                .await
                .map_err(|_| anyhow!("{timeout_ms} ms 内没有完成反射查询"))?
            })?
        }
    };
    Ok(MethodInfo::of(&md))
}

/// 用 .proto 校验方法与请求消息（validate / dry-run 用，不连服务）
pub fn check_offline(method: &str, messages: &[String], source: &Source) -> Result<MethodInfo> {
    let (svc, name) = split_method(method)?;
    let md = find_method(&proto_pool(source)?, &svc, &name, source)?;
    for m in messages {
        parse_message(&md.input(), m)?;
    }
    Ok(MethodInfo::of(&md))
}

/// 等价的 grpcurl 命令（复现 / 复制给同事；不执行）
pub fn grpcurl_command(c: &GrpcCall, root: Option<&Path>) -> String {
    use super::shell_quote;
    let mut argv: Vec<String> = vec!["grpcurl".into()];
    match parse_target(&c.target) {
        Ok(t) if !t.tls => argv.push("-plaintext".into()),
        Ok(_) if c.insecure => argv.push("-insecure".into()),
        _ => {}
    }
    if let Source::Protos { files, includes } = &c.source {
        let rel = |p: &Path| root.and_then(|r| p.strip_prefix(r).ok()).map(|x| x.display().to_string()).unwrap_or_else(|| p.display().to_string());
        for i in includes {
            let r = rel(i);
            argv.push("-import-path".into());
            argv.push(if r.is_empty() { ".".into() } else { r });
        }
        for f in files {
            argv.push("-proto".into());
            argv.push(includes.iter().find_map(|i| f.strip_prefix(i).ok()).map(|x| x.display().to_string()).unwrap_or_else(|| rel(f)));
        }
    }
    argv.push("-max-time".into());
    argv.push(format!("{}", c.timeout_ms.div_ceil(1000)));
    for (k, v) in &c.metadata {
        argv.push("-H".into());
        argv.push(format!("{k}: {v}"));
    }
    let data = match c.messages.as_slice() {
        [one] => one.clone(),
        many => many.join("\n"),
    };
    if !data.trim().is_empty() && data.trim() != "{}" {
        argv.push("-d".into());
        argv.push(data);
    }
    argv.push(parse_target(&c.target).map(|t| t.authority).unwrap_or_else(|_| c.target.clone()));
    argv.push(c.method.clone());
    argv.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------- 连接

/// 全部 gRPC 调用共用的运行时：连接（Channel 的后台任务）挂在它上面，才能跨用例复用
fn shared_runtime() -> Result<&'static tokio::runtime::Runtime> {
    static RT: std::sync::OnceLock<std::result::Result<tokio::runtime::Runtime, String>> = std::sync::OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("rb-grpc").enable_all().build().map_err(|e| e.to_string())
    })
    .as_ref()
    .map_err(|e| anyhow!("创建异步运行时失败：{e}"))
}

/// 空闲超过这么久的连接不再复用：避免撞上被网关 / NAT 悄悄断掉的半开连接
const CHANNEL_IDLE: Duration = Duration::from_secs(30);

/// 地址、TLS、是否跳过证书校验、经哪个代理（同一地址换了代理要重新连）
type ChannelKey = (String, bool, bool, String);

fn channel_cache() -> &'static Mutex<HashMap<ChannelKey, (Instant, Channel)>> {
    static C: std::sync::OnceLock<Mutex<HashMap<ChannelKey, (Instant, Channel)>>> = std::sync::OnceLock::new();
    C.get_or_init(Default::default)
}

/// 同一地址的用例复用一条 HTTP/2 连接（与 HTTP 引擎复用连接同理）：省掉每个用例的 TCP + TLS 握手。
/// 服务端关掉了连接时，下一次调用由 tonic 自动重连
async fn channel(target: &Target, insecure: bool, proxy: &super::proxy::ProxyPolicy, timeout: Duration) -> Result<Channel> {
    let via = proxy.route(target.host(), target.tls).map(|p| p.url()).unwrap_or_default();
    let key = (target.uri(), target.tls, insecure, via);
    {
        let mut cache = channel_cache().lock().unwrap_or_else(|e| e.into_inner());
        cache.retain(|_, (used, _)| used.elapsed() < CHANNEL_IDLE);
        if let Some((used, ch)) = cache.get_mut(&key) {
            *used = Instant::now();
            return Ok(ch.clone());
        }
    }
    let ch = connect(target, insecure, proxy, timeout).await?;
    channel_cache().lock().unwrap_or_else(|e| e.into_inner()).insert(key, (Instant::now(), ch.clone()));
    Ok(ch)
}

async fn connect(target: &Target, insecure: bool, proxy: &super::proxy::ProxyPolicy, timeout: Duration) -> Result<Channel> {
    // 不设 Endpoint 级超时：连接被不同超时的用例共用，每次调用的超时由调用方自己控制
    let dial = timeout.min(Duration::from_secs(10));
    let endpoint = Endpoint::from_shared(target.uri()).map_err(|e| anyhow!("gRPC 地址无效：{e}"))?.connect_timeout(dial);
    let cfg = if target.tls { Some(tls::client_config(insecure, &[b"h2"])?) } else { None };
    let host = target.host().to_string();
    let port: u16 = target.authority.rsplit_once(':').and_then(|(_, p)| p.parse().ok()).unwrap_or(if target.tls { 443 } else { 80 });
    let (tls_on, proxy) = (target.tls, proxy.clone());
    // TCP 自己拨（直连或经代理），TLS 在其上握手
    let res = endpoint
        .connect_with_connector(tower::service_fn(move |_: http::Uri| {
            let (cfg, host, proxy) = (cfg.clone(), host.clone(), proxy.clone());
            async move {
                let tcp = super::proxy::connect_async(proxy, host.clone(), port, tls_on, dial).await?;
                tcp.set_nodelay(true).ok();
                match cfg {
                    Some(cfg) => {
                        let name = rustls::pki_types::ServerName::try_from(host).map_err(|e| std::io::Error::other(e.to_string()))?;
                        let tls = tokio_rustls::TlsConnector::from(cfg).connect(name, tcp).await?;
                        Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(Box::new(tls) as Box<dyn Io>))
                    }
                    None => Ok(hyper_util::rt::TokioIo::new(Box::new(tcp) as Box<dyn Io>)),
                }
            }
        }))
        .await;
    res.map_err(|e| anyhow!(explain_connect_error(target, insecure, &e)))
}

/// 明文 / TLS 连接统一成一种类型交给 tonic
trait Io: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin> Io for T {}

/// 连接错误 → 人话（与 HTTP 的说法一致）
fn explain_connect_error(target: &Target, insecure: bool, e: &(dyn std::error::Error + 'static)) -> String {
    let mut chain = vec![e.to_string()];
    let mut src = e.source();
    let mut io_kind = None;
    while let Some(s) = src {
        if let Some(io) = s.downcast_ref::<std::io::Error>() {
            io_kind = Some(io.kind());
        }
        chain.push(s.to_string());
        src = s.source();
    }
    let raw = chain.last().cloned().unwrap_or_default();
    let all = chain.join(" / ").to_lowercase();
    let what = match io_kind {
        Some(std::io::ErrorKind::ConnectionRefused) => format!("连不上 {}：服务没有启动，或端口 / 地址不对", target.authority),
        Some(std::io::ErrorKind::TimedOut) => format!("连接 {} 超时：网络不通，或防火墙拦住了", target.authority),
        _ if all.contains("dns") || all.contains("lookup") || all.contains("nodename") => {
            format!("找不到主机 {}：域名写错了，或当前网络 / VPN 访问不到它", target.host())
        }
        _ if target.tls && (all.contains("certificate") || all.contains("tls") || all.contains("handshake")) => {
            return tls::explain_tls_error(target.host(), insecure, &raw);
        }
        _ => format!("连接 {} 失败", target.authority),
    };
    format!("{what}（{raw}）")
}

// ---------------------------------------------------------------- 描述

type PoolCache = Mutex<HashMap<String, (Instant, DescriptorPool)>>;

/// .proto 编译缓存：文件列表 + 修改时间不变就复用
fn proto_cache() -> &'static PoolCache {
    static C: std::sync::OnceLock<PoolCache> = std::sync::OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 反射结果缓存：同一地址 30 秒内复用（一次运行里同一服务的多个用例只查一次）
fn reflect_cache() -> &'static PoolCache {
    static C: std::sync::OnceLock<PoolCache> = std::sync::OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

const REFLECT_TTL: Duration = Duration::from_secs(30);

/// 编译 .proto 为描述池
pub fn proto_pool(source: &Source) -> Result<DescriptorPool> {
    let Source::Protos { files, includes } = source else { bail!("没有配置 .proto 文件") };
    let mut key = format!("{includes:?}");
    for f in files {
        let mtime = std::fs::metadata(f).and_then(|m| m.modified()).map(|t| format!("{t:?}")).unwrap_or_default();
        key.push_str(&format!("|{}@{mtime}", f.display()));
    }
    if let Some((_, p)) = proto_cache().lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return Ok(p.clone());
    }
    let mut compiler = protox::Compiler::new(includes).context("初始化 .proto 编译失败")?;
    compiler.include_imports(true);
    for f in files {
        compiler.open_file(f).map_err(|e| anyhow!("编译 {} 失败：{e}", f.display()))?;
    }
    let pool = compiler.descriptor_pool();
    proto_cache().lock().unwrap_or_else(|e| e.into_inner()).insert(key, (Instant::now(), pool.clone()));
    Ok(pool)
}

fn find_method(pool: &DescriptorPool, svc: &str, name: &str, source: &Source) -> Result<MethodDescriptor> {
    let service = pool.get_service_by_name(svc).ok_or_else(|| {
        let known: Vec<String> = pool.services().map(|s| s.full_name().to_string()).filter(|s| !s.starts_with("grpc.reflection.")).collect();
        anyhow!("{}里没有服务 {svc}（有：{}）", source.describe(), if known.is_empty() { "无".into() } else { known.join("、") })
    })?;
    let found = service.methods().find(|m| m.name() == name);
    found.ok_or_else(|| {
        let known: Vec<String> = service.methods().map(|m| m.name().to_string()).collect();
        anyhow!("服务 {svc} 没有方法 {name}（有：{}）", known.join("、"))
    })
}

async fn resolve_method(channel: &Channel, target: &Target, source: &Source, svc: &str, name: &str) -> Result<MethodDescriptor> {
    match source {
        Source::Protos { .. } => find_method(&proto_pool(source)?, svc, name, source),
        Source::Reflection => {
            let cache_key = target.authority.clone();
            let cached = reflect_cache().lock().unwrap_or_else(|e| e.into_inner()).get(&cache_key).filter(|(at, p)| at.elapsed() < REFLECT_TTL && p.get_service_by_name(svc).is_some()).map(|(_, p)| p.clone());
            let pool = match cached {
                Some(p) => p,
                None => {
                    let mut pool = reflect_cache().lock().unwrap_or_else(|e| e.into_inner()).get(&cache_key).filter(|(at, _)| at.elapsed() < REFLECT_TTL).map(|(_, p)| p.clone()).unwrap_or_default();
                    reflect_symbol(channel, svc, &mut pool).await?;
                    reflect_cache().lock().unwrap_or_else(|e| e.into_inner()).insert(cache_key, (Instant::now(), pool.clone()));
                    pool
                }
            };
            find_method(&pool, svc, name, source)
        }
    }
}

async fn reflect_all(channel: &Channel, target: &Target) -> Result<DescriptorPool> {
    let services = reflection::list_services(channel).await?;
    let mut pool = DescriptorPool::new();
    for s in services.iter().filter(|s| !s.starts_with("grpc.reflection.")) {
        reflect_symbol(channel, s, &mut pool).await?;
    }
    reflect_cache().lock().unwrap_or_else(|e| e.into_inner()).insert(target.authority.clone(), (Instant::now(), pool.clone()));
    Ok(pool)
}

/// 取包含 symbol 的文件及其全部依赖，加入描述池
async fn reflect_symbol(channel: &Channel, symbol: &str, pool: &mut DescriptorPool) -> Result<()> {
    let mut files: HashMap<String, prost_types::FileDescriptorProto> = HashMap::new();
    let mut queue: Vec<Vec<u8>> = reflection::file_containing_symbol(channel, symbol).await?;
    let mut requested: Vec<String> = vec![];
    loop {
        for raw in queue.drain(..) {
            let fd = prost_types::FileDescriptorProto::decode(raw.as_slice()).context("服务端反射返回的描述无法解析")?;
            files.entry(fd.name().to_string()).or_insert(fd);
        }
        let missing: Vec<String> = files
            .values()
            .flat_map(|f| f.dependency.iter().cloned())
            .filter(|d| !files.contains_key(d) && pool.get_file_by_name(d).is_none() && !requested.contains(d))
            .collect();
        if missing.is_empty() {
            break;
        }
        for d in missing {
            requested.push(d.clone());
            match reflection::file_by_filename(channel, &d).await {
                Ok(raw) => queue.extend(raw),
                // 常见的 google/protobuf/*.proto 服务端可能不提供：用内置的
                Err(_) if d.starts_with("google/protobuf/") => {}
                Err(e) => return Err(e),
            }
        }
    }
    let wkt = DescriptorPool::global();
    let mut protos: Vec<prost_types::FileDescriptorProto> = vec![];
    for f in files.values() {
        for d in &f.dependency {
            if !files.contains_key(d) && pool.get_file_by_name(d).is_none() {
                if let Some(w) = wkt.get_file_by_name(d) {
                    protos.push(w.file_descriptor_proto().clone());
                }
            }
        }
    }
    protos.extend(files.into_values());
    pool.add_file_descriptor_protos(protos).map_err(|e| anyhow!("服务端反射返回的描述不完整：{e}"))?;
    Ok(())
}

mod reflection {
    //! 反射客户端：先试 v1，服务端只支持 v1alpha（老版本 grpc-go 等）时回落
    use anyhow::{anyhow, Result};
    use tonic::transport::Channel;

    macro_rules! client {
        ($m:ident, $v:path) => {
            mod $m {
                use $v as pb;
                use pb::server_reflection_client::ServerReflectionClient;
                use pb::server_reflection_request::MessageRequest;
                use pb::server_reflection_response::MessageResponse;
                use tonic::transport::Channel;

                pub enum Answer {
                    Files(Vec<Vec<u8>>),
                    Services(Vec<String>),
                }

                pub async fn ask(channel: &Channel, req: MessageRequest) -> Result<Answer, tonic::Status> {
                    let mut client = ServerReflectionClient::new(channel.clone());
                    let r = pb::ServerReflectionRequest { host: String::new(), message_request: Some(req) };
                    let mut stream = client.server_reflection_info(tokio_stream::iter(vec![r])).await?.into_inner();
                    let msg = stream.message().await?.ok_or_else(|| tonic::Status::unknown("反射没有返回内容"))?;
                    match msg.message_response {
                        Some(MessageResponse::FileDescriptorResponse(f)) => Ok(Answer::Files(f.file_descriptor_proto)),
                        Some(MessageResponse::ListServicesResponse(l)) => Ok(Answer::Services(l.service.into_iter().map(|s| s.name).collect())),
                        Some(MessageResponse::ErrorResponse(e)) => Err(tonic::Status::new(tonic::Code::from(e.error_code), e.error_message)),
                        _ => Err(tonic::Status::unknown("反射返回了意外的内容")),
                    }
                }

                pub fn list() -> MessageRequest {
                    MessageRequest::ListServices(String::new())
                }
                pub fn symbol(s: &str) -> MessageRequest {
                    MessageRequest::FileContainingSymbol(s.to_string())
                }
                pub fn file(f: &str) -> MessageRequest {
                    MessageRequest::FileByFilename(f.to_string())
                }
            }
        };
    }
    client!(v1, tonic_reflection::pb::v1);
    client!(v1alpha, tonic_reflection::pb::v1alpha);

    enum Q<'a> {
        List,
        Symbol(&'a str),
        File(&'a str),
    }

    async fn ask(channel: &Channel, q: Q<'_>) -> Result<(Vec<Vec<u8>>, Vec<String>)> {
        let first = match &q {
            Q::List => v1::ask(channel, v1::list()).await,
            Q::Symbol(s) => v1::ask(channel, v1::symbol(s)).await,
            Q::File(f) => v1::ask(channel, v1::file(f)).await,
        };
        let res = match first {
            Err(st) if st.code() == tonic::Code::Unimplemented => {
                let r = match &q {
                    Q::List => v1alpha::ask(channel, v1alpha::list()).await,
                    Q::Symbol(s) => v1alpha::ask(channel, v1alpha::symbol(s)).await,
                    Q::File(f) => v1alpha::ask(channel, v1alpha::file(f)).await,
                };
                r.map(|a| match a {
                    v1alpha::Answer::Files(f) => (f, vec![]),
                    v1alpha::Answer::Services(s) => (vec![], s),
                })
            }
            other => other.map(|a| match a {
                v1::Answer::Files(f) => (f, vec![]),
                v1::Answer::Services(s) => (vec![], s),
            }),
        };
        res.map_err(|st| match st.code() {
            tonic::Code::Unimplemented => anyhow!("服务端没有开启反射（grpc.reflection）：在 requestbaby.json 的 grpc.protos 里配置 .proto 文件"),
            tonic::Code::NotFound => match q {
                Q::Symbol(s) => anyhow!("服务端反射里没有 {s}：服务名写错了，或服务没有注册"),
                Q::File(f) => anyhow!("服务端反射里没有文件 {f}"),
                Q::List => anyhow!("{}", st.message()),
            },
            _ => anyhow!("反射查询失败：{} {}", crate::config::protocols::grpc_code_name(st.code() as u16), st.message()),
        })
    }

    pub async fn list_services(channel: &Channel) -> Result<Vec<String>> {
        Ok(ask(channel, Q::List).await?.1)
    }
    pub async fn file_containing_symbol(channel: &Channel, s: &str) -> Result<Vec<Vec<u8>>> {
        Ok(ask(channel, Q::Symbol(s)).await?.0)
    }
    pub async fn file_by_filename(channel: &Channel, f: &str) -> Result<Vec<Vec<u8>>> {
        Ok(ask(channel, Q::File(f)).await?.0)
    }
}

// ---------------------------------------------------------------- 消息

fn json_options() -> SerializeOptions {
    SerializeOptions::new().stringify_64_bit_integers(false).use_proto_field_name(true).skip_default_fields(false)
}

/// JSON → 动态消息（未知字段报错，提示可用字段）
pub fn parse_message(desc: &MessageDescriptor, json: &str) -> Result<DynamicMessage> {
    let mut de = serde_json::Deserializer::from_str(json);
    let msg = DynamicMessage::deserialize(desc.clone(), &mut de).map_err(|e| {
        let fields: Vec<String> = desc.fields().map(|f| f.name().to_string()).collect();
        anyhow!("请求消息不符合 {}：{e}（可用字段：{}）", desc.full_name(), fields.join("、"))
    })?;
    de.end().map_err(|e| anyhow!("请求消息不是合法 JSON：{e}"))?;
    Ok(msg)
}

pub fn message_json(m: &DynamicMessage) -> Value {
    m.serialize_with_options(serde_json::value::Serializer, &json_options()).unwrap_or(Value::Null)
}

/// 请求骨架：每个字段填默认值（嵌套消息展开 3 层；oneof 只取第一个分支）
pub fn template(desc: &MessageDescriptor, depth: usize) -> Value {
    let mut obj = serde_json::Map::new();
    let mut seen_oneofs: Vec<String> = vec![];
    for f in desc.fields() {
        if let Some(o) = f.containing_oneof() {
            if seen_oneofs.contains(&o.name().to_string()) {
                continue;
            }
            seen_oneofs.push(o.name().to_string());
        }
        let v = if f.is_map() {
            Value::Object(Default::default())
        } else {
            let one = match f.kind() {
                Kind::Message(m) => match m.full_name() {
                    "google.protobuf.Timestamp" => Value::String("1970-01-01T00:00:00Z".into()),
                    "google.protobuf.Duration" => Value::String("0s".into()),
                    "google.protobuf.Struct" | "google.protobuf.Any" => Value::Object(Default::default()),
                    "google.protobuf.Value" => Value::Null,
                    n if n.starts_with("google.protobuf.") && n.ends_with("Value") => scalar_default(&m.fields().next().map(|x| x.kind())),
                    _ if depth >= 3 => Value::Object(Default::default()),
                    _ => template(&m, depth + 1),
                },
                k => scalar_default(&Some(k)),
            };
            if f.is_list() { Value::Array(vec![one]) } else { one }
        };
        obj.insert(f.name().to_string(), v);
    }
    Value::Object(obj)
}

fn scalar_default(kind: &Option<Kind>) -> Value {
    match kind {
        Some(Kind::Bool) => Value::Bool(false),
        Some(Kind::String) => Value::String(String::new()),
        Some(Kind::Bytes) => Value::String(String::new()),
        Some(Kind::Double | Kind::Float) => serde_json::json!(0.0),
        Some(Kind::Enum(e)) => e.values().next().map(|v| Value::String(v.name().to_string())).unwrap_or(Value::Null),
        Some(Kind::Message(_)) | None => Value::Null,
        Some(_) => serde_json::json!(0),
    }
}

// ---------------------------------------------------------------- 调用

#[derive(Clone)]
struct DynCodec {
    output: MessageDescriptor,
    received: Arc<AtomicU64>,
}

struct DynEncoder;
struct DynDecoder(MessageDescriptor, Arc<AtomicU64>);

impl Codec for DynCodec {
    type Encode = DynamicMessage;
    type Decode = DynamicMessage;
    type Encoder = DynEncoder;
    type Decoder = DynDecoder;

    fn encoder(&mut self) -> Self::Encoder {
        DynEncoder
    }
    fn decoder(&mut self) -> Self::Decoder {
        DynDecoder(self.output.clone(), self.received.clone())
    }
}

impl Encoder for DynEncoder {
    type Item = DynamicMessage;
    type Error = tonic::Status;
    fn encode(&mut self, item: Self::Item, dst: &mut EncodeBuf<'_>) -> Result<(), Self::Error> {
        item.encode(dst).map_err(|e| tonic::Status::internal(format!("编码请求失败：{e}")))
    }
}

impl Decoder for DynDecoder {
    type Item = DynamicMessage;
    type Error = tonic::Status;
    fn decode(&mut self, src: &mut DecodeBuf<'_>) -> Result<Option<Self::Item>, Self::Error> {
        use prost::bytes::Buf;
        self.1.fetch_add(src.remaining() as u64, Ordering::Relaxed);
        DynamicMessage::decode(self.0.clone(), src).map(Some).map_err(|e| tonic::Status::internal(format!("响应消息无法按 {} 解析：{e}", self.0.full_name())))
    }
}

fn metadata_pairs(m: &MetadataMap) -> Vec<(String, String)> {
    use base64::Engine;
    m.iter()
        .map(|kv| match kv {
            KeyAndValueRef::Ascii(k, v) => (k.to_string(), v.to_str().unwrap_or_default().to_string()),
            KeyAndValueRef::Binary(k, v) => (k.to_string(), base64::engine::general_purpose::STANDARD.encode(v.as_encoded_bytes())),
        })
        .collect()
}

fn request_metadata(pairs: &[(String, String)]) -> Result<MetadataMap> {
    let mut map = MetadataMap::new();
    for (k, v) in pairs {
        let key = k.trim().to_ascii_lowercase();
        if key.ends_with("-bin") {
            let name = tonic::metadata::BinaryMetadataKey::from_bytes(key.as_bytes()).map_err(|_| anyhow!("元数据名 {k} 不合法"))?;
            map.append_bin(name, tonic::metadata::BinaryMetadataValue::from_bytes(v.as_bytes()));
        } else {
            let name = tonic::metadata::AsciiMetadataKey::from_bytes(key.as_bytes()).map_err(|_| anyhow!("元数据名 {k} 不合法（只能用小写字母、数字、- _ .）"))?;
            let value = v.parse().map_err(|_| anyhow!("元数据 {k} 的值含有不能放进请求头的字符"))?;
            map.append(name, value);
        }
    }
    Ok(map)
}

async fn invoke(channel: Channel, md: &MethodDescriptor, c: &GrpcCall, timeout: Duration, started: Instant) -> Result<GrpcReply> {
    let input = md.input();
    let msgs: Vec<DynamicMessage> = c.messages.iter().map(|m| parse_message(&input, m)).collect::<Result<_>>()?;
    if !md.is_client_streaming() && msgs.len() != 1 {
        bail!("{} 是{}方法，只能发一条消息（用 message，不用 messages）", c.method, kind_label(false, md.is_server_streaming()));
    }
    let received = Arc::new(AtomicU64::new(0));
    let codec = DynCodec { output: md.output(), received: received.clone() };
    let path: http::uri::PathAndQuery = format!("/{}/{}", md.parent_service().full_name(), md.name()).parse().map_err(|e| anyhow!("方法路径无效：{e}"))?;
    let metadata = request_metadata(&c.metadata)?;
    let mut reply = GrpcReply { server_streaming: md.is_server_streaming(), sent: msgs.iter().map(message_json).collect(), ..Default::default() };

    let mut client = tonic::client::Grpc::new(channel);
    client.ready().await.map_err(|e| anyhow!("gRPC 连接不可用：{e}"))?;
    fn with_meta<T>(mut r: tonic::Request<T>, metadata: &MetadataMap, timeout: Duration) -> tonic::Request<T> {
        *r.metadata_mut() = metadata.clone();
        r.set_timeout(timeout);
        r
    }

    let outcome: Result<(), tonic::Status> = match (md.is_client_streaming(), md.is_server_streaming()) {
        (false, false) => {
            let req = with_meta(tonic::Request::new(msgs.into_iter().next().unwrap()), &metadata, timeout);
            client.unary(req, path, codec).await.map(|resp| {
                reply.metadata = metadata_pairs(resp.metadata());
                reply.messages.push(message_json(resp.get_ref()));
            })
        }
        (true, false) => {
            let req = with_meta(tonic::Request::new(tokio_stream::iter(msgs)), &metadata, timeout);
            client.client_streaming(req, path, codec).await.map(|resp| {
                reply.metadata = metadata_pairs(resp.metadata());
                reply.messages.push(message_json(resp.get_ref()));
            })
        }
        (false, true) => {
            let req = with_meta(tonic::Request::new(msgs.into_iter().next().unwrap()), &metadata, timeout);
            match client.server_streaming(req, path, codec).await {
                Ok(resp) => collect_stream(resp, &mut reply).await,
                Err(st) => Err(st),
            }
        }
        (true, true) => {
            let req = with_meta(tonic::Request::new(tokio_stream::iter(msgs)), &metadata, timeout);
            match client.streaming(req, path, codec).await {
                Ok(resp) => collect_stream(resp, &mut reply).await,
                Err(st) => Err(st),
            }
        }
    };
    if let Err(st) = outcome {
        if is_transport_failure(&st) {
            // 复用的连接断了、重连又失败：与首次连接失败同一说法
            let src = std::error::Error::source(&st);
            let connect_failed = st.message().contains("connect") || grpc_status_source(&st).contains("connect");
            if let (true, Some(src), Ok(target)) = (connect_failed, src, parse_target(&c.target)) {
                bail!(explain_connect_error(&target, c.insecure, src));
            }
            bail!("gRPC 调用中断：{}（{}）", st.message(), grpc_status_source(&st));
        }
        reply.code = st.code() as u16;
        reply.status_message = st.message().to_string();
        let mut meta = metadata_pairs(st.metadata());
        meta.retain(|(k, _)| k != "grpc-status" && k != "grpc-message");
        reply.metadata.extend(meta);
    }
    reply.metadata.retain(|(k, _)| k != "grpc-status" && k != "grpc-message");
    reply.metadata.push(("grpc-status".into(), reply.code.to_string()));
    if !reply.status_message.is_empty() {
        reply.metadata.push(("grpc-message".into(), reply.status_message.clone()));
    }
    reply.time_ms = started.elapsed().as_millis() as u64;
    reply.size = received.load(Ordering::Relaxed);
    Ok(reply)
}

async fn collect_stream(resp: tonic::Response<tonic::Streaming<DynamicMessage>>, reply: &mut GrpcReply) -> Result<(), tonic::Status> {
    reply.metadata = metadata_pairs(resp.metadata());
    let mut stream = resp.into_inner();
    while let Some(m) = stream.message().await? {
        reply.messages.push(message_json(&m));
    }
    if let Some(t) = stream.trailers().await? {
        reply.metadata.extend(metadata_pairs(&t));
    }
    Ok(())
}

/// 传输层失败（连接被重置、协议错误）不是服务端给出的状态
fn is_transport_failure(st: &tonic::Status) -> bool {
    use std::error::Error;
    st.code() == tonic::Code::Unavailable && st.source().is_some() && st.metadata().is_empty()
        || st.code() == tonic::Code::Unknown && st.message().contains("h2 protocol error")
}

fn grpc_status_source(st: &tonic::Status) -> String {
    use std::error::Error;
    let mut s = st.source();
    let mut last = String::new();
    while let Some(e) = s {
        last = e.to_string();
        s = e.source();
    }
    if last.is_empty() { st.code().description().to_string() } else { last }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROTO: &str = r#"
syntax = "proto3";
package shop.v1;
import "google/protobuf/timestamp.proto";
enum State { STATE_UNSPECIFIED = 0; PAID = 1; }
message Item { string sku = 1; int32 qty = 2; }
message GetOrderRequest { int64 id = 1; }
message Order {
  int64 id = 1;
  repeated Item items = 2;
  State state = 3;
  map<string, string> labels = 4;
  google.protobuf.Timestamp created_at = 5;
  oneof payer { string user = 6; string company = 7; }
}
service OrderService {
  rpc GetOrder(GetOrderRequest) returns (Order);
  rpc Watch(GetOrderRequest) returns (stream Order);
  rpc Upload(stream Item) returns (Order);
  rpc Chat(stream Item) returns (stream Item);
}
"#;

    fn source(dir: &Path) -> Source {
        std::fs::create_dir_all(dir.join("protos/shop")).unwrap();
        std::fs::write(dir.join("protos/shop/order.proto"), PROTO).unwrap();
        let cfg = crate::config::GrpcConfig { protos: vec!["protos".into()], import_paths: vec![] };
        Source::from_config(dir, &cfg).unwrap()
    }

    #[test]
    fn targets_parse_with_tls_and_ports() {
        assert_eq!(parse_target("localhost:50051").unwrap(), Target { tls: false, authority: "localhost:50051".into() });
        assert_eq!(parse_target("grpcs://api.example.com").unwrap(), Target { tls: true, authority: "api.example.com:443".into() });
        assert_eq!(parse_target("https://h:8443/x").unwrap(), Target { tls: true, authority: "h:8443".into() });
        assert!(parse_target("grpc://h").unwrap_err().to_string().contains("缺少端口"));
        assert!(parse_target("ws://h:1").is_err());
        assert_eq!(split_method("a.b.Svc/Get").unwrap(), ("a.b.Svc".into(), "Get".into()));
        assert_eq!(split_method("a.b.Svc.Get").unwrap(), ("a.b.Svc".into(), "Get".into()));
        assert!(split_method("Get").is_err());
    }

    #[test]
    fn protos_compile_and_describe_methods() {
        let dir = tempfile::tempdir().unwrap();
        let src = source(dir.path());
        let methods = list_methods("", false, 1000, &src, &Default::default()).unwrap();
        let names: Vec<(&str, &str)> = methods.iter().map(|m| (m.method.as_str(), m.kind_label())).collect();
        assert_eq!(
            names,
            [
                ("shop.v1.OrderService/Chat", "双向流"),
                ("shop.v1.OrderService/GetOrder", "一元"),
                ("shop.v1.OrderService/Upload", "客户端流"),
                ("shop.v1.OrderService/Watch", "服务端流"),
            ]
        );
        let up = describe_method("", "shop.v1.OrderService/Upload", false, 1000, &src, &Default::default()).unwrap();
        assert_eq!(up.template, serde_json::json!({"sku": "", "qty": 0}));
        // 骨架：嵌套展开、枚举取第一个值、map 为空对象、oneof 只取第一个分支、时间戳写 RFC 3339
        let pool = proto_pool(&src).unwrap();
        let order = pool.get_message_by_name("shop.v1.Order").unwrap();
        assert_eq!(
            template(&order, 0),
            serde_json::json!({"id": 0, "items": [{"sku": "", "qty": 0}], "state": "STATE_UNSPECIFIED", "labels": {}, "created_at": "1970-01-01T00:00:00Z", "user": ""})
        );
    }

    #[test]
    fn messages_round_trip_as_plain_json() {
        let dir = tempfile::tempdir().unwrap();
        let pool = proto_pool(&source(dir.path())).unwrap();
        let order = pool.get_message_by_name("shop.v1.Order").unwrap();
        // 输入接受 .proto 名或 lowerCamelCase；int64 写数字或字符串都行
        let m = parse_message(&order, r#"{"id":"9007199254740993","items":[{"sku":"A","qty":2}],"state":"PAID","createdAt":"2026-01-02T03:04:05Z"}"#).unwrap();
        let out = message_json(&m);
        // 输出：字段名同 .proto、int64 是数字（不丢精度）、默认值也输出（code = 0 这类断言可写）
        assert_eq!(out["id"], serde_json::json!(9007199254740993_i64));
        assert_eq!(out["items"][0], serde_json::json!({"sku": "A", "qty": 2}));
        assert_eq!(out["state"], "PAID");
        assert_eq!(out["created_at"], "2026-01-02T03:04:05Z");
        assert_eq!(out["labels"], serde_json::json!({}));
        let e = parse_message(&order, r#"{"idd":1}"#).unwrap_err().to_string();
        assert!(e.contains("idd") && e.contains("可用字段：id、items"), "{e}");
    }

    #[test]
    fn offline_check_reports_unknown_methods() {
        let dir = tempfile::tempdir().unwrap();
        let src = source(dir.path());
        assert!(check_offline("shop.v1.OrderService/GetOrder", &[r#"{"id":1}"#.into()], &src).is_ok());
        let e = check_offline("shop.v1.OrderService/Get", &["{}".into()], &src).unwrap_err().to_string();
        assert!(e.contains("没有方法 Get") && e.contains("GetOrder"), "{e}");
        let e = check_offline("shop.v1.Nope/Get", &["{}".into()], &src).unwrap_err().to_string();
        assert!(e.contains("没有服务 shop.v1.Nope") && e.contains("shop.v1.OrderService"), "{e}");
    }

    #[test]
    fn grpcurl_command_reproduces_the_call() {
        let dir = tempfile::tempdir().unwrap();
        let src = source(dir.path());
        let call = GrpcCall {
            target: "localhost:50051".into(),
            method: "shop.v1.OrderService/GetOrder".into(),
            messages: vec![r#"{"id":1}"#.into()],
            metadata: vec![("authorization".into(), "Bearer t".into())],
            timeout_ms: 1500,
            insecure: false,
            source: src,
            proxy: Default::default(),
        };
        let root = dir.path();
        assert_eq!(
            grpcurl_command(&call, Some(root)),
            "grpcurl -plaintext -import-path protos -import-path . -proto shop/order.proto -max-time 2 -H 'authorization: Bearer t' -d '{\"id\":1}' localhost:50051 shop.v1.OrderService/GetOrder"
        );
    }

    #[test]
    fn refused_connection_is_explained() {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let c = GrpcCall {
            target: format!("127.0.0.1:{port}"),
            method: "a.B/C".into(),
            messages: vec!["{}".into()],
            metadata: vec![],
            timeout_ms: 2000,
            insecure: false,
            source: Source::Reflection,
            proxy: Default::default(),
        };
        let e = call(&c).unwrap_err().to_string();
        assert!(e.starts_with(&format!("连不上 127.0.0.1:{port}")), "{e}");
    }
}
