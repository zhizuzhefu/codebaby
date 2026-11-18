//! 代理：所有对外连接（HTTP / gRPC / WebSocket / 浏览器 / adb）走同一套规则。
//!
//! - 未配置：跟随系统——先看环境变量（HTTPS_PROXY / HTTP_PROXY / ALL_PROXY / NO_PROXY），
//!   没有再看操作系统的代理设置（macOS：系统设置 → 网络 → 代理），都没有则直连。
//!   这样从 Dock 打开的桌面端（拿不到 shell 里的环境变量）与命令行结论一致
//! - `proxy.enabled: false`：全部直连，环境变量也不用
//! - `proxy.enabled: true`：全部经 `proxy.url`（socks5 / socks5h / http），`noProxy` 里的主机直连
//!
//! 本机地址（localhost / 127.x / ::1）始终直连：经代理访问到的会是代理那台机器。
//! libcurl 原生支持代理；其余连接用这里的拨号器（SOCKS5 / HTTP CONNECT）。
//! 本机浏览器无法给 SOCKS5 代理传账号密码，所以给它一个进程内的本地中转（[`Relay`]）；
//! adb 不支持代理，网络设备 / 远程 adb server 经本地转发端口（[`forward`]）连接。
use anyhow::{anyhow, bail, Context, Result};
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// 环境变量：`off` 全部直连；代理地址则启用它（CLI `--no-proxy` / `--proxy` 即设置它）
pub const ENV_OVERRIDE: &str = "REQUESTBABY_PROXY";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Http,
    /// 本机解析域名，把 IP 交给代理
    Socks5,
    /// 域名交给代理解析（内网域名、被污染的 DNS 都靠它）
    Socks5h,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proxy {
    pub kind: Kind,
    pub host: String,
    pub port: u16,
    pub user: Option<String>,
    pub pass: Option<String>,
    /// 直连的主机
    pub no_proxy: Vec<String>,
}

/// 一次运行的代理规则
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ProxyPolicy {
    /// 跟随系统：环境变量 → 操作系统代理设置 → 直连
    #[default]
    System,
    /// 全部直连
    Off,
    On(Proxy),
}

impl Proxy {
    pub fn parse(url: &str) -> Result<Self> {
        let url = url.trim();
        let (scheme, rest) = url.split_once("://").ok_or_else(|| anyhow!("代理地址 {} 缺少协议：写 socks5://主机:端口 或 http://主机:端口", redact(url)))?;
        let kind = match scheme.to_ascii_lowercase().as_str() {
            "http" => Kind::Http,
            "socks5" | "socks" => Kind::Socks5,
            "socks5h" => Kind::Socks5h,
            other => bail!("代理协议 {other} 不支持：用 socks5 / socks5h / http"),
        };
        let rest = rest.trim_end_matches('/');
        let (auth, hostport) = match rest.rsplit_once('@') {
            Some((a, h)) => (Some(a), h),
            None => (None, rest),
        };
        let (host, port) = if let Some(v6) = hostport.strip_prefix('[') {
            let (h, p) = v6.split_once(']').ok_or_else(|| anyhow!("代理地址 {} 不对", redact(url)))?;
            (h.to_string(), p.strip_prefix(':'))
        } else {
            match hostport.rsplit_once(':') {
                Some((h, p)) => (h.to_string(), Some(p)),
                None => (hostport.to_string(), None),
            }
        };
        if host.is_empty() || host.contains('/') {
            bail!("代理地址 {} 不对：写 socks5://主机:端口", redact(url));
        }
        let port = match port {
            Some(p) => p.parse().map_err(|_| anyhow!("代理地址 {} 的端口不对", redact(url)))?,
            None => 1080,
        };
        let (user, pass) = match auth {
            Some(a) => match a.split_once(':') {
                Some((u, p)) => (Some(pct_decode(u)), Some(pct_decode(p))),
                None => (Some(pct_decode(a)), None),
            },
            None => (None, None),
        };
        Ok(Self { kind, host, port, user, pass, no_proxy: vec![] })
    }

    fn scheme(&self) -> &'static str {
        match self.kind {
            Kind::Http => "http",
            Kind::Socks5 => "socks5",
            Kind::Socks5h => "socks5h",
        }
    }

    fn host_port(&self) -> String {
        if self.host.contains(':') { format!("[{}]:{}", self.host, self.port) } else { format!("{}:{}", self.host, self.port) }
    }

    /// 带账号密码的完整地址（交给 libcurl）
    pub fn url(&self) -> String {
        let auth = match (&self.user, &self.pass) {
            (Some(u), Some(p)) => format!("{}:{}@", pct_encode(u), pct_encode(p)),
            (Some(u), None) => format!("{}@", pct_encode(u)),
            _ => String::new(),
        };
        format!("{}://{auth}{}", self.scheme(), self.host_port())
    }

    /// 显示用：不含账号密码
    pub fn display(&self) -> String {
        format!("{}://{}", self.scheme(), self.host_port())
    }

    /// 等价 curl 命令的 `--noproxy`（libcurl 的写法：.example.com 匹配子域）
    pub fn curl_no_proxy(&self) -> String {
        let mut list: Vec<String> = ["localhost", "127.0.0.1", "::1"].map(String::from).to_vec();
        list.extend(self.no_proxy.iter().map(|h| h.trim().trim_start_matches("*.").to_string()).filter(|h| !h.is_empty()));
        list.join(",")
    }
}

/// 地址里的账号密码换成 ***
pub fn redact(url: &str) -> String {
    match url.split_once("://") {
        Some((s, rest)) => match rest.rsplit_once('@') {
            Some((_, h)) => format!("{s}://***@{h}"),
            None => url.to_string(),
        },
        None => url.to_string(),
    }
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn pct_encode(s: &str) -> String {
    s.bytes()
        .map(|c| if c.is_ascii_alphanumeric() || b"-._~".contains(&c) { (c as char).to_string() } else { format!("%{c:02X}") })
        .collect()
}

/// `host:port` / `[v6]:port` / `host` → (主机, 端口)
pub fn split_host_port(hp: &str, default_port: u16) -> (String, u16) {
    if let Some(rest) = hp.strip_prefix('[') {
        let (h, p) = rest.split_once(']').unwrap_or((rest, ""));
        return (h.to_string(), p.strip_prefix(':').and_then(|p| p.parse().ok()).unwrap_or(default_port));
    }
    match hp.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => (h.to_string(), p.parse().unwrap_or(default_port)),
        _ => (hp.to_string(), default_port),
    }
}

pub fn is_loopback(host: &str) -> bool {
    let h = host.trim_matches(['[', ']']).to_ascii_lowercase();
    h == "localhost" || h.ends_with(".localhost") || h.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// noProxy 规则：`*` 全部；`example.com` / `.example.com` / `*.example.com` 含子域；IP；IPv4 网段 `10.0.0.0/8`
pub fn bypassed(rules: &[String], host: &str) -> bool {
    let host = host.trim_matches(['[', ']']).trim_end_matches('.').to_ascii_lowercase();
    rules.iter().any(|r| {
        let r = r.trim().to_ascii_lowercase();
        if r.is_empty() {
            return false;
        }
        if r == "*" {
            return true;
        }
        if let Some((net, bits)) = r.split_once('/') {
            if let (Ok(IpAddr::V4(net)), Ok(bits), Ok(IpAddr::V4(ip))) = (net.parse::<IpAddr>(), bits.parse::<u32>(), host.parse::<IpAddr>()) {
                let mask = if bits == 0 { 0 } else { u32::MAX << (32 - bits.min(32)) };
                return u32::from(net) & mask == u32::from(ip) & mask;
            }
            return false;
        }
        let d = r.trim_start_matches("*.").trim_start_matches('.');
        host == d || host.ends_with(&format!(".{d}"))
    })
}

impl ProxyPolicy {
    /// 项目配置 + 环境变量覆盖 → 规则。`render` 替换 url 里的 {{变量}}
    pub fn resolve(cfg: Option<&crate::config::ProxyConfig>, render: impl Fn(&str) -> String) -> Result<Self> {
        let no_proxy = cfg.map(|c| c.no_proxy.clone()).unwrap_or_default();
        if let Some(v) = std::env::var(ENV_OVERRIDE).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) {
            return match v.to_ascii_lowercase().as_str() {
                "off" | "none" | "direct" | "0" | "false" => Ok(Self::Off),
                "env" | "system" => Ok(Self::System),
                _ => Ok(Self::On(Proxy { no_proxy, ..Proxy::parse(&v).with_context(|| format!("环境变量 {ENV_OVERRIDE}"))? })),
            };
        }
        let Some(cfg) = cfg else { return Ok(Self::System) };
        if !cfg.enabled {
            return Ok(Self::Off);
        }
        let url = render(&cfg.url);
        if url.trim().is_empty() {
            bail!("requestbaby.json 的 proxy 已启用但没写 url（socks5://主机:端口）");
        }
        if url.contains("{{") {
            bail!("proxy.url 里的变量没有定义：{}", redact(&url));
        }
        Ok(Self::On(Proxy { no_proxy, ..Proxy::parse(&url).context("requestbaby.json 的 proxy.url")? }))
    }

    /// 连接 host 用哪个代理（None = 直连）。`tls` 只在跟随系统时用来选 HTTPS / HTTP 代理
    pub fn route(&self, host: &str, tls: bool) -> Option<Proxy> {
        if is_loopback(host) {
            return None;
        }
        match self {
            Self::Off => None,
            Self::On(p) => (!bypassed(&p.no_proxy, host)).then(|| p.clone()),
            Self::System => match env_proxy(tls) {
                Some((_, url, no)) => {
                    if bypassed(&no, host) {
                        return None;
                    }
                    Proxy::parse(&url).ok()
                }
                None if env_proxy_set() => None,
                None => system_settings().route(host, tls),
            },
        }
    }

    /// 访问这个地址用哪个代理（按地址里的主机与协议判断）
    pub fn route_url(&self, url: &str) -> Option<Proxy> {
        let (scheme, rest) = url.split_once("://").unwrap_or(("http", url));
        let hp = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let hp = hp.rsplit_once('@').map(|(_, h)| h).unwrap_or(hp);
        let (host, _) = split_host_port(hp, 0);
        let tls = matches!(scheme.to_ascii_lowercase().as_str(), "https" | "wss" | "grpcs");
        self.route(&host, tls)
    }

    /// 跟随系统时代理从哪来（给人看）
    fn system_source(&self) -> String {
        if let Some((name, url, _)) = env_proxy(true).or_else(|| env_proxy(false)) {
            return format!("跟随系统：环境变量 {name}（{}）", Proxy::parse(&url).map(|p| p.display()).unwrap_or_else(|_| redact(&url)));
        }
        let sys = system_settings();
        let found: Vec<String> = [("HTTP", &sys.http, Kind::Http), ("HTTPS", &sys.https, Kind::Http), ("SOCKS", &sys.socks, Kind::Socks5h)]
            .into_iter()
            .filter_map(|(label, hp, kind)| hp.as_ref().map(|(h, p)| format!("{label} {}", Proxy { kind, host: h.clone(), port: *p, user: None, pass: None, no_proxy: vec![] }.display())))
            .collect();
        match (found.is_empty(), sys.pac) {
            (false, _) => format!("跟随系统：系统代理设置（{}）", found.join(" · ")),
            (true, true) => "跟随系统：系统使用 PAC 自动配置，requestbaby 暂不支持，按直连处理（Web 用例的浏览器仍按 PAC）".into(),
            (true, false) => "跟随系统：没有设置代理，直连".into(),
        }
    }

    /// 给人看的说明
    pub fn describe(&self) -> String {
        match self {
            Self::System => self.system_source(),
            Self::Off => "已禁用（全部直连）".into(),
            Self::On(p) => {
                let extra = if p.no_proxy.is_empty() { String::new() } else { format!("，{} 直连", p.no_proxy.join(" / ")) };
                format!("{}{extra}", p.display())
            }
        }
    }
}

fn env_var(names: &[&'static str]) -> Option<(&'static str, String)> {
    names.iter().find_map(|n| std::env::var(n).ok().filter(|v| !v.trim().is_empty()).map(|v| (*n, v)))
}

fn env_proxy_set() -> bool {
    env_var(&["https_proxy", "HTTPS_PROXY", "http_proxy", "HTTP_PROXY", "all_proxy", "ALL_PROXY"]).is_some()
}

/// 环境变量里的代理：(变量名, 地址, NO_PROXY)
fn env_proxy(tls: bool) -> Option<(&'static str, String, Vec<String>)> {
    let (name, url) = if tls { env_var(&["https_proxy", "HTTPS_PROXY"]) } else { env_var(&["http_proxy", "HTTP_PROXY"]) }.or_else(|| env_var(&["all_proxy", "ALL_PROXY"]))?;
    let no = env_var(&["no_proxy", "NO_PROXY"]).map(|(_, v)| v.split(',').map(String::from).collect()).unwrap_or_default();
    // 环境变量里常见不带协议的 host:port，按 http 代理理解
    let url = if url.contains("://") { url } else { format!("http://{url}") };
    Some((name, url, no))
}

/// 操作系统的代理设置（目前读 macOS；其他系统以环境变量为准）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SystemSettings {
    pub http: Option<(String, u16)>,
    pub https: Option<(String, u16)>,
    pub socks: Option<(String, u16)>,
    /// 例外：主机 / 域名通配 / 网段；`<local>` = 不带点的主机名
    pub exceptions: Vec<String>,
    pub exclude_simple: bool,
    /// 用了 PAC 自动配置（暂不支持）
    pub pac: bool,
}

impl SystemSettings {
    fn route(&self, host: &str, tls: bool) -> Option<Proxy> {
        let simple = !host.contains('.') && !host.contains(':');
        if (simple && (self.exclude_simple || self.exceptions.iter().any(|e| e == "<local>"))) || bypassed(&self.exceptions, host) {
            return None;
        }
        let web = if tls { &self.https } else { &self.http };
        let (kind, (h, p)) = match (web, &self.socks) {
            (Some(hp), _) => (Kind::Http, hp.clone()),
            (None, Some(hp)) => (Kind::Socks5h, hp.clone()),
            _ => return None,
        };
        Some(Proxy { kind, host: h, port: p, user: None, pass: None, no_proxy: vec![] })
    }
}

/// 系统设置的快照：几秒内复用（每个连接都要查，系统设置改了很快生效）
fn system_settings() -> SystemSettings {
    static CACHE: OnceLock<Mutex<Option<(std::time::Instant, SystemSettings)>>> = OnceLock::new();
    let mut c = CACHE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, s)) = c.as_ref() {
        if at.elapsed() < Duration::from_secs(3) {
            return s.clone();
        }
    }
    let s = read_system_settings();
    *c = Some((std::time::Instant::now(), s.clone()));
    s
}

#[cfg(target_os = "macos")]
fn read_system_settings() -> SystemSettings {
    use system_configuration::core_foundation::array::CFArray;
    use system_configuration::core_foundation::base::{CFType, TCFType};
    use system_configuration::core_foundation::dictionary::CFDictionary;
    use system_configuration::core_foundation::number::CFNumber;
    use system_configuration::core_foundation::string::CFString;
    let Some(map) = system_configuration::dynamic_store::SCDynamicStoreBuilder::new("requestbaby").build().get_proxies() else {
        return SystemSettings::default();
    };
    let get = |map: &CFDictionary<CFString, CFType>, k: &str| map.find(CFString::new(k)).map(|v| v.clone());
    let num = |k: &str| get(&map, k).and_then(|v| v.downcast::<CFNumber>()).and_then(|n| n.to_i32()).unwrap_or(0);
    let pair = |enable: &str, host: &str, port: &str| {
        if num(enable) != 1 {
            return None;
        }
        let h = get(&map, host).and_then(|v| v.downcast::<CFString>()).map(|s| s.to_string()).filter(|s| !s.is_empty())?;
        let p = u16::try_from(num(port)).ok().filter(|p| *p > 0).unwrap_or(8080);
        Some((h, p))
    };
    let exceptions = get(&map, "ExceptionsList")
        .and_then(|v| v.downcast::<CFArray>())
        .map(|a| {
            a.iter()
                .filter_map(|ptr| unsafe { CFType::wrap_under_get_rule(*ptr as system_configuration::core_foundation::base::CFTypeRef) }.downcast::<CFString>().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    SystemSettings {
        http: pair("HTTPEnable", "HTTPProxy", "HTTPPort"),
        https: pair("HTTPSEnable", "HTTPSProxy", "HTTPSPort"),
        socks: pair("SOCKSEnable", "SOCKSProxy", "SOCKSPort"),
        exceptions,
        exclude_simple: num("ExcludeSimpleHostnames") == 1,
        pac: num("ProxyAutoConfigEnable") == 1 || num("ProxyAutoDiscoveryEnable") == 1,
    }
}

#[cfg(not(target_os = "macos"))]
fn read_system_settings() -> SystemSettings {
    SystemSettings::default()
}

/// 跟随系统时，系统设置是否用了 PAC（浏览器自己能处理 PAC，此时不给它指定代理）
pub fn system_uses_pac() -> bool {
    system_settings().pac
}

// ---------------------------------------------------------------------------------------------
// 拨号
// ---------------------------------------------------------------------------------------------

fn dial_direct(host: &str, port: u16, timeout: Duration) -> std::io::Result<TcpStream> {
    let mut last = None;
    for addr in (host.trim_matches(['[', ']']), port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(s) => return Ok(s),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, format!("无法解析 {host}"))))
}

/// 按规则建立到 host:port 的 TCP 连接（直连或经代理），之后的读写与直连无异
pub fn connect(policy: &ProxyPolicy, host: &str, port: u16, tls: bool, timeout: Duration) -> Result<TcpStream> {
    let host = host.trim_matches(['[', ']']);
    match policy.route(host, tls) {
        None => dial_direct(host, port, timeout).with_context(|| format!("连接 {host}:{port} 失败")),
        Some(p) => via(&p, host, port, timeout),
    }
}

/// tokio 版：拨号在阻塞线程里做，完成后交给运行时
pub async fn connect_async(policy: ProxyPolicy, host: String, port: u16, tls: bool, timeout: Duration) -> std::io::Result<tokio::net::TcpStream> {
    let s = tokio::task::spawn_blocking(move || connect(&policy, &host, port, tls, timeout))
        .await
        .map_err(std::io::Error::other)?
        .map_err(|e| {
            // 保留底层错误类型（拒绝连接 / 超时…），上层据此给出说明
            let kind = e.chain().find_map(|c| c.downcast_ref::<std::io::Error>()).map(|io| io.kind()).unwrap_or(std::io::ErrorKind::Other);
            std::io::Error::new(kind, format!("{e:#}"))
        })?;
    s.set_nonblocking(true)?;
    s.set_read_timeout(None)?;
    s.set_write_timeout(None)?;
    tokio::net::TcpStream::from_std(s)
}

fn via(p: &Proxy, host: &str, port: u16, timeout: Duration) -> Result<TcpStream> {
    let mut s = dial_direct(&p.host, p.port, timeout).with_context(|| format!("连接代理 {} 失败", p.display()))?;
    s.set_read_timeout(Some(timeout)).ok();
    s.set_write_timeout(Some(timeout)).ok();
    s.set_nodelay(true).ok();
    let r = match p.kind {
        Kind::Http => http_connect(&mut s, p, host, port),
        Kind::Socks5 | Kind::Socks5h => socks5_connect(&mut s, p, host, port),
    };
    r.with_context(|| format!("经代理 {} 连接 {host}:{port} 失败", p.display()))?;
    s.set_read_timeout(None).ok();
    s.set_write_timeout(None).ok();
    Ok(s)
}

fn http_connect(s: &mut TcpStream, p: &Proxy, host: &str, port: u16) -> Result<()> {
    use base64::Engine;
    let target = if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") };
    let mut req = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n");
    if let Some(u) = &p.user {
        let token = base64::engine::general_purpose::STANDARD.encode(format!("{u}:{}", p.pass.as_deref().unwrap_or_default()));
        req.push_str(&format!("Proxy-Authorization: Basic {token}\r\n"));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes())?;
    // 逐字节读到空行：不能多读，后面就是隧道里的数据
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if s.read(&mut b)? == 0 {
            bail!("代理关闭了连接");
        }
        head.push(b[0]);
        if head.len() > 16 * 1024 {
            bail!("代理的回复过长");
        }
    }
    let line = String::from_utf8_lossy(&head).lines().next().unwrap_or_default().to_string();
    let code = line.split_whitespace().nth(1).unwrap_or_default();
    match code {
        "200" => Ok(()),
        "407" => bail!("代理要求认证（{line}）：在代理地址里写 user:pass@"),
        _ => bail!("代理拒绝：{line}"),
    }
}

/// 代理本身能否连上（SOCKS5 还会完成认证，账号密码错了在这里就能发现）
pub fn probe(p: &Proxy, timeout: Duration) -> Result<()> {
    let mut s = dial_direct(&p.host, p.port, timeout).with_context(|| format!("连不上代理 {}", p.display()))?;
    s.set_read_timeout(Some(timeout)).ok();
    s.set_write_timeout(Some(timeout)).ok();
    if matches!(p.kind, Kind::Socks5 | Kind::Socks5h) {
        socks5_auth(&mut s, p).with_context(|| format!("代理 {}", p.display()))?;
    }
    Ok(())
}

fn socks5_connect(s: &mut TcpStream, p: &Proxy, host: &str, port: u16) -> Result<()> {
    socks5_auth(s, p)?;
    socks5_request(s, p, host, port)
}

fn socks5_auth(s: &mut TcpStream, p: &Proxy) -> Result<()> {
    let auth = p.user.is_some();
    s.write_all(if auth { &[5, 2, 0, 2] } else { &[5, 1, 0] })?;
    let mut r = [0u8; 2];
    s.read_exact(&mut r)?;
    if r[0] != 5 {
        bail!("对方不是 SOCKS5 代理");
    }
    match r[1] {
        0 => {}
        2 => {
            let u = p.user.as_deref().unwrap_or_default().as_bytes();
            let pw = p.pass.as_deref().unwrap_or_default().as_bytes();
            let mut m = vec![1, u.len().min(255) as u8];
            m.extend_from_slice(&u[..u.len().min(255)]);
            m.push(pw.len().min(255) as u8);
            m.extend_from_slice(&pw[..pw.len().min(255)]);
            s.write_all(&m)?;
            s.read_exact(&mut r)?;
            if r[1] != 0 {
                bail!("SOCKS5 代理认证失败：账号或密码不对");
            }
        }
        0xFF if !auth => bail!("SOCKS5 代理要求认证：在代理地址里写 user:pass@"),
        _ => bail!("SOCKS5 代理不接受这种认证方式"),
    }
    Ok(())
}

fn socks5_request(s: &mut TcpStream, p: &Proxy, host: &str, port: u16) -> Result<()> {
    let mut req = vec![5, 1, 0];
    let ip = match p.kind {
        Kind::Socks5 => match host.parse::<IpAddr>() {
            Ok(ip) => Some(ip),
            Err(_) => Some((host, port).to_socket_addrs().with_context(|| format!("本机解析 {host} 失败（要让代理解析域名用 socks5h://）"))?.next().ok_or_else(|| anyhow!("本机解析 {host} 失败"))?.ip()),
        },
        _ => host.parse::<IpAddr>().ok(),
    };
    match ip {
        Some(IpAddr::V4(v4)) => {
            req.push(1);
            req.extend_from_slice(&v4.octets());
        }
        Some(IpAddr::V6(v6)) => {
            req.push(4);
            req.extend_from_slice(&v6.octets());
        }
        None => {
            req.push(3);
            req.push(host.len().min(255) as u8);
            req.extend_from_slice(&host.as_bytes()[..host.len().min(255)]);
        }
    }
    req.extend_from_slice(&port.to_be_bytes());
    s.write_all(&req)?;
    let mut head = [0u8; 4];
    s.read_exact(&mut head)?;
    if head[1] != 0 {
        let why = match head[1] {
            1 => "代理内部错误",
            2 => "代理规则不允许",
            3 => "网络不可达",
            4 => "主机不可达",
            5 => "连接被拒绝",
            6 => "超时",
            7 => "命令不支持",
            8 => "地址类型不支持",
            _ => "未知错误",
        };
        bail!("SOCKS5 代理：{why}");
    }
    let skip = match head[3] {
        1 => 4,
        4 => 16,
        3 => {
            let mut l = [0u8; 1];
            s.read_exact(&mut l)?;
            l[0] as usize
        }
        _ => bail!("SOCKS5 代理回复格式不对"),
    };
    let mut rest = vec![0u8; skip + 2];
    s.read_exact(&mut rest)?;
    Ok(())
}

/// 双向转发，任一方向结束就关闭两端
fn splice(a: TcpStream, b: TcpStream) {
    let (Ok(mut a2), Ok(mut b2)) = (a.try_clone(), b.try_clone()) else { return };
    let (mut a, mut b) = (a, b);
    let t = std::thread::spawn(move || {
        let _ = std::io::copy(&mut a2, &mut b2);
        let _ = b2.shutdown(std::net::Shutdown::Both);
        let _ = a2.shutdown(std::net::Shutdown::Both);
    });
    let _ = std::io::copy(&mut b, &mut a);
    let _ = a.shutdown(std::net::Shutdown::Both);
    let _ = b.shutdown(std::net::Shutdown::Both);
    let _ = t.join();
}

const DIAL: Duration = Duration::from_secs(20);

// ---------------------------------------------------------------------------------------------
// 本地中转：给本机浏览器一个不用认证的 SOCKS5 入口，出口按规则直连或经代理
// ---------------------------------------------------------------------------------------------

pub struct Relay {
    pub addr: SocketAddr,
    stop: Arc<AtomicBool>,
}

impl Relay {
    pub fn start(policy: ProxyPolicy) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").context("启动本地代理中转失败")?;
        let addr = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                if stop2.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(c) = conn else { continue };
                let policy = policy.clone();
                std::thread::spawn(move || {
                    let _ = serve_socks(c, &policy);
                });
            }
        });
        Ok(Self { addr, stop })
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // 唤醒阻塞在 accept 上的线程
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(200));
    }
}

fn serve_socks(mut c: TcpStream, policy: &ProxyPolicy) -> Result<()> {
    c.set_read_timeout(Some(DIAL)).ok();
    let mut h = [0u8; 2];
    c.read_exact(&mut h)?;
    let mut methods = vec![0u8; h[1] as usize];
    c.read_exact(&mut methods)?;
    if h[0] != 5 || !methods.contains(&0) {
        c.write_all(&[5, 0xFF])?;
        bail!("不是 SOCKS5 请求");
    }
    c.write_all(&[5, 0])?;
    let mut req = [0u8; 4];
    c.read_exact(&mut req)?;
    let host = match req[3] {
        1 => {
            let mut a = [0u8; 4];
            c.read_exact(&mut a)?;
            IpAddr::from(a).to_string()
        }
        4 => {
            let mut a = [0u8; 16];
            c.read_exact(&mut a)?;
            IpAddr::from(a).to_string()
        }
        3 => {
            let mut l = [0u8; 1];
            c.read_exact(&mut l)?;
            let mut d = vec![0u8; l[0] as usize];
            c.read_exact(&mut d)?;
            String::from_utf8_lossy(&d).into_owned()
        }
        _ => bail!("地址类型不支持"),
    };
    let mut pb = [0u8; 2];
    c.read_exact(&mut pb)?;
    let port = u16::from_be_bytes(pb);
    if req[1] != 1 {
        c.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0])?;
        bail!("只支持 CONNECT");
    }
    match connect(policy, &host, port, port == 443, DIAL) {
        Ok(up) => {
            c.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0])?;
            c.set_read_timeout(None).ok();
            splice(c, up);
            Ok(())
        }
        Err(e) => {
            let _ = c.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]);
            Err(e)
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 本地转发端口：127.0.0.1:端口 → 目标（经代理）。adb 不支持代理，靠它连网络设备 / 远程 adb server
// ---------------------------------------------------------------------------------------------

type ForwardKey = (String, String, u16);

fn forwards() -> &'static Mutex<std::collections::HashMap<ForwardKey, SocketAddr>> {
    static F: OnceLock<Mutex<std::collections::HashMap<ForwardKey, SocketAddr>>> = OnceLock::new();
    F.get_or_init(Default::default)
}

/// 到 host:port 的本地转发地址（进程内同一目标复用）。端口由目标决定，多次运行得到同一个，
/// adb 里看到的设备序列号因此稳定
pub fn forward(proxy: &Proxy, host: &str, port: u16) -> Result<SocketAddr> {
    let key = (proxy.url(), host.to_string(), port);
    let mut map = forwards().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(a) = map.get(&key) {
        return Ok(*a);
    }
    let seed = key.0.bytes().chain(host.bytes()).chain(port.to_be_bytes()).fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    let mut listener = None;
    for i in 0..50u32 {
        let p = 40000 + ((seed.wrapping_add(i)) % 10000) as u16;
        if let Ok(l) = TcpListener::bind(("127.0.0.1", p)) {
            listener = Some(l);
            break;
        }
    }
    let listener = match listener {
        Some(l) => l,
        None => TcpListener::bind("127.0.0.1:0").context("启动本地转发失败")?,
    };
    let addr = listener.local_addr()?;
    let policy = ProxyPolicy::On(Proxy { no_proxy: vec![], ..proxy.clone() });
    let target = host.to_string();
    std::thread::spawn(move || {
        for c in listener.incoming().flatten() {
            let (policy, target) = (policy.clone(), target.clone());
            std::thread::spawn(move || {
                if let Ok(up) = connect(&policy, &target, port, false, DIAL) {
                    splice(c, up);
                }
            });
        }
    });
    map.insert(key, addr);
    Ok(addr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proxy_urls() {
        let p = Proxy::parse("socks5h://u%40x:p%3Aw@10.0.0.1:1081").unwrap();
        assert_eq!((p.kind, p.host.as_str(), p.port), (Kind::Socks5h, "10.0.0.1", 1081));
        assert_eq!((p.user.as_deref(), p.pass.as_deref()), (Some("u@x"), Some("p:w")));
        assert_eq!(p.url(), "socks5h://u%40x:p%3Aw@10.0.0.1:1081");
        assert_eq!(p.display(), "socks5h://10.0.0.1:1081");
        assert_eq!(Proxy::parse("http://[::1]:8080").unwrap().host_port(), "[::1]:8080");
        assert_eq!(Proxy::parse("socks5://proxy").unwrap().port, 1080);
        assert!(Proxy::parse("127.0.0.1:1080").is_err());
        assert!(Proxy::parse("https://p:1").is_err());
        assert_eq!(redact("socks5://u:p@h:1"), "socks5://***@h:1");
    }

    #[test]
    fn bypass_rules() {
        let r: Vec<String> = ["corp.internal", "*.svc", ".lan", "10.0.0.0/8", "192.168.1.5"].map(String::from).to_vec();
        assert!(bypassed(&r, "corp.internal") && bypassed(&r, "api.corp.internal"));
        assert!(bypassed(&r, "a.b.svc") && bypassed(&r, "nas.lan"));
        assert!(bypassed(&r, "10.2.3.4") && !bypassed(&r, "11.2.3.4"));
        assert!(bypassed(&r, "192.168.1.5") && !bypassed(&r, "example.com"));
        assert!(!bypassed(&r, "notcorp.internal"));
        assert!(bypassed(&["*".into()], "anything"));
        assert!(is_loopback("localhost") && is_loopback("127.0.0.5") && is_loopback("[::1]") && !is_loopback("10.0.0.1"));
    }

    #[test]
    fn policy_routes() {
        let p = Proxy { no_proxy: vec!["corp".into()], ..Proxy::parse("socks5://h:1").unwrap() };
        let on = ProxyPolicy::On(p);
        assert!(on.route("example.com", true).is_some());
        assert!(on.route("x.corp", true).is_none());
        assert!(on.route("127.0.0.1", false).is_none(), "本机地址始终直连");
        assert!(ProxyPolicy::Off.route("example.com", true).is_none());
    }

    #[test]
    fn resolves_project_setting() {
        if std::env::var(ENV_OVERRIDE).is_ok() {
            return;
        }
        let cfg = |enabled: bool, url: &str| crate::config::ProxyConfig { enabled, url: url.into(), no_proxy: vec!["corp".into()] };
        let render = |s: &str| s.replace("{{proxyPass}}", "s3cret");
        assert_eq!(ProxyPolicy::resolve(None, render).unwrap(), ProxyPolicy::System);
        assert_eq!(ProxyPolicy::resolve(Some(&cfg(false, "socks5://h:1")), render).unwrap(), ProxyPolicy::Off);
        let ProxyPolicy::On(p) = ProxyPolicy::resolve(Some(&cfg(true, "socks5://me:{{proxyPass}}@h:1")), render).unwrap() else { panic!() };
        assert_eq!((p.pass.as_deref(), p.no_proxy.as_slice()), (Some("s3cret"), ["corp".to_string()].as_slice()));
        assert!(ProxyPolicy::resolve(Some(&cfg(true, "")), render).is_err());
        assert!(ProxyPolicy::resolve(Some(&cfg(true, "socks5://{{nope}}:1")), render).is_err());
        assert!(!ProxyPolicy::On(p).describe().contains("s3cret"));
    }

    #[test]
    fn system_settings_route() {
        let sys = SystemSettings {
            http: Some(("10.0.0.2".into(), 7890)),
            https: None,
            socks: Some(("10.0.0.2".into(), 7891)),
            exceptions: vec!["*.local".into(), "192.168.0.0/16".into(), "<local>".into()],
            exclude_simple: false,
            pac: false,
        };
        assert_eq!(sys.route("example.com", false).map(|p| p.display()), Some("http://10.0.0.2:7890".into()));
        assert_eq!(sys.route("example.com", true).map(|p| p.display()), Some("socks5h://10.0.0.2:7891".into()), "没有 HTTPS 代理时用 SOCKS");
        assert!(sys.route("nas.local", false).is_none());
        assert!(sys.route("192.168.3.4", false).is_none());
        assert!(sys.route("intranet", false).is_none(), "<local> = 不带点的主机名");
        assert!(SystemSettings::default().route("example.com", true).is_none());
    }
}
