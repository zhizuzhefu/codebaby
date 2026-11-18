//! Cookie 自动管理：像浏览器一样保存响应的 `Set-Cookie`，后续同域请求自动带上 `Cookie`。
//!
//! - 作用域：一次运行（CLI run / workflow）内共享；Desktop 在本次会话内跨发送保持
//! - 用例显式写了 `Cookie` 请求头时以用例为准，不自动附加
//! - 项目 `"cookies": false` 关闭
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    /// 不含前导点；host_only 时只匹配完全相同的主机
    pub domain: String,
    #[serde(skip)]
    pub host_only: bool,
    pub path: String,
    #[serde(skip)]
    pub secure: bool,
    /// 过期时间（毫秒时间戳）；None = 会话 Cookie
    #[serde(skip)]
    pub expires_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CookieJar {
    cookies: Vec<Cookie>,
}

/// URL 拆分：(https?, host 小写, path)
fn split_url(url: &str) -> Option<(bool, String, String)> {
    let (scheme, rest) = url.split_once("://")?;
    let (authority, path) = match rest.find(['/', '?', '#']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let host = authority.rsplit('@').next()?.split(':').next()?.to_lowercase();
    let path = path.split(['?', '#']).next().filter(|p| p.starts_with('/')).unwrap_or("/").to_string();
    Some((scheme.eq_ignore_ascii_case("https"), host, path))
}

/// RFC 6265 默认路径：请求路径去掉最后一段
fn default_path(path: &str) -> String {
    match path.rfind('/') {
        Some(0) | None => "/".into(),
        Some(i) => path[..i].to_string(),
    }
}

fn path_matches(request: &str, cookie: &str) -> bool {
    request == cookie
        || (request.starts_with(cookie) && (cookie.ends_with('/') || request[cookie.len()..].starts_with('/')))
}

fn domain_matches(host: &str, c: &Cookie) -> bool {
    if c.host_only {
        host == c.domain
    } else {
        host == c.domain || host.ends_with(&format!(".{}", c.domain))
    }
}

impl CookieJar {
    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }

    pub fn len(&self) -> usize {
        self.cookies.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Cookie> {
        self.cookies.iter()
    }

    pub fn clear(&mut self) {
        self.cookies.clear();
    }

    /// 合并另一个 jar（同名同域同路径以 other 为准）
    pub fn merge(&mut self, other: &CookieJar) {
        for c in &other.cookies {
            self.put(c.clone());
        }
    }

    /// 放入一个 Cookie（浏览器里得到的；同名同域同路径替换）
    pub fn insert(&mut self, c: Cookie) {
        self.put(c);
    }

    fn put(&mut self, c: Cookie) {
        self.cookies.retain(|x| !(x.name == c.name && x.domain == c.domain && x.path == c.path));
        self.cookies.push(c);
    }

    /// 保存一次响应的全部 Set-Cookie
    pub fn store<'a>(&mut self, url: &str, set_cookies: impl IntoIterator<Item = &'a str>, now_ms: u64) {
        let Some((_, host, path)) = split_url(url) else { return };
        for header in set_cookies {
            let mut parts = header.split(';');
            let Some((name, value)) = parts.next().and_then(|nv| nv.split_once('=')) else { continue };
            let name = name.trim().to_string();
            if name.is_empty() {
                continue;
            }
            let mut c = Cookie {
                name,
                value: value.trim().trim_matches('"').to_string(),
                domain: host.clone(),
                host_only: true,
                path: default_path(&path),
                secure: false,
                expires_at_ms: None,
            };
            let mut max_age: Option<i64> = None;
            for attr in parts {
                let (k, v) = attr.split_once('=').map(|(k, v)| (k.trim(), v.trim())).unwrap_or((attr.trim(), ""));
                match k.to_ascii_lowercase().as_str() {
                    "domain" if !v.is_empty() => {
                        let d = v.trim_start_matches('.').to_lowercase();
                        // 只接受当前主机或其父域
                        if host == d || host.ends_with(&format!(".{d}")) {
                            c.domain = d;
                            c.host_only = false;
                        }
                    }
                    "path" if v.starts_with('/') => c.path = v.to_string(),
                    "secure" => c.secure = true,
                    "max-age" => max_age = v.parse().ok(),
                    "expires" if max_age.is_none() => c.expires_at_ms = parse_http_date(v),
                    _ => {}
                }
            }
            if let Some(secs) = max_age {
                c.expires_at_ms = Some(if secs <= 0 { 0 } else { now_ms + secs as u64 * 1000 });
            }
            // 已过期 = 服务端删除该 Cookie
            if c.expires_at_ms.is_some_and(|t| t <= now_ms) {
                self.cookies.retain(|x| !(x.name == c.name && x.domain == c.domain && x.path == c.path));
            } else {
                self.put(c);
            }
        }
    }

    /// 该 URL 应携带的 Cookie 请求头（路径更长的在前）
    pub fn header_for(&self, url: &str, now_ms: u64) -> Option<String> {
        let (https, host, path) = split_url(url)?;
        let mut hits: Vec<&Cookie> = self
            .cookies
            .iter()
            .filter(|c| domain_matches(&host, c) && path_matches(&path, &c.path) && (https || !c.secure))
            .filter(|c| c.expires_at_ms.is_none_or(|t| t > now_ms))
            .collect();
        if hits.is_empty() {
            return None;
        }
        hits.sort_by_key(|c| std::cmp::Reverse(c.path.len()));
        Some(hits.iter().map(|c| format!("{}={}", c.name, c.value)).collect::<Vec<_>>().join("; "))
    }
}

/// `Expires` 属性（IMF-fixdate，如 `Wed, 21 Oct 2015 07:28:00 GMT`）→ 毫秒时间戳；无法解析视为会话 Cookie
fn parse_http_date(s: &str) -> Option<u64> {
    let parts: Vec<&str> = s.split([' ', ',', '-', ':']).filter(|p| !p.is_empty()).collect();
    // [Wed, 21, Oct, 2015, 07, 28, 00, GMT]
    if parts.len() < 7 {
        return None;
    }
    let day: u64 = parts[1].parse().ok()?;
    let month = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"]
        .iter()
        .position(|m| parts[2].to_ascii_lowercase().starts_with(m))? as u64
        + 1;
    let mut year: u64 = parts[3].parse().ok()?;
    if year < 100 {
        year += if year < 70 { 2000 } else { 1900 };
    }
    let (h, mi, sec): (u64, u64, u64) = (parts[4].parse().ok()?, parts[5].parse().ok()?, parts[6].parse().ok()?);
    // 公历日期 → 自 1970-01-01 的天数（Howard Hinnant days_from_civil）
    let (y, m) = if month <= 2 { (year - 1, month + 9) } else { (year, month - 3) };
    let era = y / 400;
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = (era * 146097 + doe).checked_sub(719468)?;
    Some(((days * 86400) + h * 3600 + mi * 60 + sec) * 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_700_000_000_000;

    #[test]
    fn stores_and_sends_back_like_a_browser() {
        let mut jar = CookieJar::default();
        jar.store(
            "https://api.example.com/auth/login",
            ["SESSION=abc; Path=/; HttpOnly; Secure", "theme=dark", "track=1; Domain=example.com; Path=/"],
            NOW,
        );
        // theme 默认路径 /auth；SESSION 仅 https
        assert_eq!(jar.header_for("https://api.example.com/auth/me", NOW).unwrap(), "theme=dark; SESSION=abc; track=1");
        assert_eq!(jar.header_for("https://api.example.com/orders", NOW).unwrap(), "SESSION=abc; track=1");
        assert_eq!(jar.header_for("http://api.example.com/orders", NOW).unwrap(), "track=1");
        // 父域 Cookie 对子域生效；host-only 不外溢
        assert_eq!(jar.header_for("https://www.example.com/", NOW).unwrap(), "track=1");
        assert!(jar.header_for("https://other.com/", NOW).is_none());
    }

    #[test]
    fn server_can_update_and_delete() {
        let mut jar = CookieJar::default();
        jar.store("http://h/x", ["a=1; Path=/"], NOW);
        jar.store("http://h/x", ["a=2; Path=/"], NOW);
        assert_eq!(jar.header_for("http://h/", NOW).unwrap(), "a=2");
        jar.store("http://h/x", ["a=; Path=/; Max-Age=0"], NOW);
        assert!(jar.is_empty());
        jar.store("http://h/x", ["b=1; Path=/; Expires=Wed, 21 Oct 2015 07:28:00 GMT"], NOW);
        assert!(jar.is_empty(), "已过期的 Expires 视为删除");
        jar.store("http://h/x", ["c=1; Path=/; Max-Age=60"], NOW);
        assert!(jar.header_for("http://h/", NOW + 61_000).is_none(), "到期后不再发送");
    }

    #[test]
    fn rejects_foreign_domain_attribute() {
        let mut jar = CookieJar::default();
        jar.store("http://api.example.com/", ["evil=1; Domain=attacker.com"], NOW);
        // 非父域的 Domain 属性被忽略，退化为 host-only
        assert!(jar.header_for("http://attacker.com/", NOW).is_none());
        assert_eq!(jar.header_for("http://api.example.com/", NOW).unwrap(), "evil=1");
    }

    #[test]
    fn http_date_parses() {
        assert_eq!(parse_http_date("Wed, 21 Oct 2015 07:28:00 GMT"), Some(1_445_412_480_000));
    }
}
