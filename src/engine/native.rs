//! Native 引擎：hyper 内置客户端（连接复用 + HTTP/2，批量执行档位）
//! 编译：`cargo build --features native`；启用：`requestbaby run --engine native`
use super::{ResolvedRequest, Response, Transport};
use anyhow::{anyhow, Result};
use bytes::Bytes;
use http_body_util::Full;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use std::sync::OnceLock;

/// 进程级 tokio Runtime（泄漏式单例：与进程同生命周期）
fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| tokio::runtime::Runtime::new().expect("tokio runtime"))
}

type HttpsClient = Client<hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>, Full<Bytes>>;

/// 进程级客户端单例（连接池复用，HTTP/2 多路复用）
fn client() -> &'static HttpsClient {
    static CLIENT: OnceLock<HttpsClient> = OnceLock::new();
    CLIENT.get_or_init(|| {
        let native = hyper_rustls::HttpsConnectorBuilder::new().with_native_roots();
        let tls = match native {
            Ok(b) => b,
            Err(_) => hyper_rustls::HttpsConnectorBuilder::new().with_webpki_roots(),
        };
        let connector = tls.https_or_http().enable_http2().build();
        Client::builder(TokioExecutor::new()).build(connector)
    })
}

/// NativeTransport
#[derive(Debug, Clone, Copy, Default)]
pub struct NativeTransport;

impl Transport for NativeTransport {
    fn execute(&self, req: &ResolvedRequest) -> Result<Response> {
        runtime().block_on(async_exec(req))
    }
}

async fn async_exec(req: &ResolvedRequest) -> Result<Response> {
    use hyper::header;
    use hyper::{Method, Request};

    let method = Method::from_bytes(req.method.as_bytes())?;
    let mut builder = Request::builder().method(method).uri(&req.url);
    for (k, v) in &req.headers {
        builder = builder.header(header::HeaderName::from_bytes(k.as_bytes())?, v.as_bytes());
    }
    if req.insecure {
        return Err(anyhow!("native 引擎暂不支持 insecure（请用 curl 引擎或补齐证书）"));
    }
    let body = req.body.clone().unwrap_or_default();
    let request = builder.body(Full::new(Bytes::from(body)))?;

    let start = std::time::Instant::now();
    let fut = client().request(request);
    let resp = tokio::time::timeout(
        std::time::Duration::from_millis(req.timeout_ms.max(1000)),
        fut,
    )
    .await
    .map_err(|_| anyhow!("native: 请求超时（{}ms）", req.timeout_ms))?
    .map_err(|e| anyhow!("native: {e}"))?;
    let status = resp.status().as_u16();
    let headers: Vec<(String, String)> = resp
        .headers()
        .iter()
        .map(|(k, v): (&hyper::header::HeaderName, &hyper::header::HeaderValue)| {
            (k.as_str().to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned())
        })
        .collect();
    let collected = http_body_util::BodyExt::collect(resp.into_body())
        .await
        .map_err(|e| anyhow!("native body: {e}"))?;
    let bytes = collected.to_bytes();
    Ok(Response {
        status,
        headers,
        size: bytes.len() as u64,
        body: bytes.to_vec(),
        time_ms: start.elapsed().as_millis() as u64,
        transport_error: None,
    })
}
