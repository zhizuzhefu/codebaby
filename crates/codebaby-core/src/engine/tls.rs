//! gRPC / WebSocket 共用的 TLS 配置：rustls + 系统平台证书校验（与 libcurl 一致：
//! macOS 钥匙串 / Windows 证书库 / Linux 系统 CA，企业或自签 CA 加进系统后同样生效）
use anyhow::{Context, Result};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use std::sync::Arc;

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

/// 客户端 TLS 配置；`insecure` 跳过证书校验（仍校验握手签名），`alpn` 如 gRPC 的 h2
pub fn client_config(insecure: bool, alpn: &[&[u8]]) -> Result<Arc<ClientConfig>> {
    let provider = provider();
    let builder = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .context("TLS 初始化失败")?;
    let verifier: Arc<dyn ServerCertVerifier> = if insecure {
        Arc::new(NoVerify(provider.clone()))
    } else {
        Arc::new(rustls_platform_verifier::Verifier::new(provider).context("读取系统证书库失败")?)
    };
    let mut cfg = builder.dangerous().with_custom_certificate_verifier(verifier).with_no_client_auth();
    cfg.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    Ok(Arc::new(cfg))
}

/// insecure：接受任意证书（自签 / 过期 / 域名不符），握手签名照常校验
#[derive(Debug)]
struct NoVerify(Arc<CryptoProvider>);

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// 证书 / 握手错误 → 人话（与 HTTP 的说法一致）
pub fn explain_tls_error(host: &str, insecure: bool, raw: &str) -> String {
    if insecure {
        format!("和 {host} 的 TLS 握手失败：端口不是 TLS，或服务端 TLS 配置有误（{raw}）")
    } else {
        format!("和 {host} 的 TLS 握手 / 证书校验失败：证书自签、过期或域名不符（测试环境可设 insecure: true）（{raw}）")
    }
}

/// 单线程 tokio 运行时（每次调用创建，结束即释放；runner 是同步的）
pub fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread().enable_all().build().context("创建异步运行时失败")
}
