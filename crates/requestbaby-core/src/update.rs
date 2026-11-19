//! 检查与获取新版本：命令行 `requestbaby upgrade` 与桌面端「有新版本」共用同一实现。
//!
//! 发布渠道只有 GitHub Release：每个版本一个 `vX.Y.Z` 标签，附件是各平台的命令行 tar.gz
//! 与签名公证过的 macOS DMG，每个附件旁有同名 `.sha256` 校验文件。
//! 检查更新只请求 GitHub 的公开接口（不带任何本机信息），走与用例相同的代理规则。
use crate::engine::proxy::ProxyPolicy;
use crate::engine::{CurlTransport, ResolvedRequest, Transport};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// 发布仓库（`owner/name`）。仓库迁移时只改这里；环境变量 REQUESTBABY_UPDATE_REPO 可临时指向别处
pub const REPO: &str = "zhizuzhefu/requestbaby";

/// 当前版本
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

pub fn repo() -> String {
    std::env::var("REQUESTBABY_UPDATE_REPO").ok().filter(|s| s.contains('/')).unwrap_or_else(|| REPO.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// v0.4.0
    pub tag: String,
    /// 0.4.0
    pub version: String,
    /// 更新说明（Markdown）
    pub notes: String,
    /// 发布页地址
    pub page: String,
    pub assets: Vec<Asset>,
}

impl Release {
    pub fn is_newer(&self) -> bool {
        is_newer(&self.version, CURRENT)
    }

    fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }

    /// 当前系统的命令行安装包
    pub fn cli_asset(&self) -> Option<&Asset> {
        self.asset(&format!("requestbaby-{}.tar.gz", target()?))
    }

    /// macOS 桌面端安装包
    pub fn dmg_asset(&self) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name.ends_with(".dmg"))
    }

    /// 附件的校验文件
    pub fn checksum_of(&self, a: &Asset) -> Option<&Asset> {
        self.asset(&format!("{}.sha256", a.name))
    }
}

/// 发布包的目标平台名；不在发布范围内的系统返回 None
pub fn target() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-musl"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-musl"),
        _ => None,
    }
}

/// `x.y.z`（可带 `v` 前缀与 `-预发布`）：latest 比 current 新才算新版本；预发布低于同号正式版
pub fn is_newer(latest: &str, current: &str) -> bool {
    fn parse(v: &str) -> Option<((u64, u64, u64), bool)> {
        let v = v.trim().trim_start_matches('v');
        let (core, pre) = match v.split_once('-') {
            Some((c, _)) => (c, true),
            None => (v.split_once('+').map(|(c, _)| c).unwrap_or(v), false),
        };
        let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
        Some(((it.next()??, it.next().flatten().unwrap_or(0), it.next().flatten().unwrap_or(0)), pre))
    }
    match (parse(latest), parse(current)) {
        (Some((l, lpre)), Some((c, cpre))) => l > c || (l == c && cpre && !lpre),
        _ => false,
    }
}

/// GitHub `releases/latest` 的响应 → Release
pub fn parse_release(json: &str) -> Result<Release> {
    let v: Value = serde_json::from_str(json).context("发布信息不是合法 JSON")?;
    let tag = v["tag_name"].as_str().ok_or_else(|| anyhow!("发布信息里没有 tag_name"))?.to_string();
    let assets = v["assets"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| Some(Asset { name: x["name"].as_str()?.to_string(), url: x["browser_download_url"].as_str()?.to_string(), size: x["size"].as_u64().unwrap_or(0) }))
                .collect()
        })
        .unwrap_or_default();
    Ok(Release {
        version: tag.trim_start_matches('v').to_string(),
        tag,
        notes: v["body"].as_str().unwrap_or_default().trim().to_string(),
        page: v["html_url"].as_str().unwrap_or_default().to_string(),
        assets,
    })
}

fn get(url: &str, accept: &str, timeout_ms: u64, proxy: &ProxyPolicy) -> Result<crate::engine::Response> {
    let req = ResolvedRequest {
        method: "GET".into(),
        url: url.into(),
        headers: vec![("User-Agent".into(), format!("requestbaby/{CURRENT}")), ("Accept".into(), accept.into())],
        timeout_ms,
        follow_redirects: true,
        proxy: proxy.clone(),
        ..Default::default()
    };
    let r = CurlTransport::new().execute(&req)?;
    if let Some(e) = &r.transport_error {
        bail!("{e}");
    }
    Ok(r)
}

/// 最新的正式版本；仓库里还没有发布时返回 None
pub fn latest(proxy: &ProxyPolicy) -> Result<Option<Release>> {
    let repo = repo();
    let r = get(&format!("https://api.github.com/repos/{repo}/releases/latest"), "application/vnd.github+json", 15_000, proxy)
        .with_context(|| "无法连接 GitHub 检查新版本（网络或代理问题）".to_string())?;
    match r.status {
        200 => parse_release(&r.body_text()).map(Some),
        404 => Ok(None),
        403 | 429 => bail!("GitHub 暂时限制了检查频率，稍后再试"),
        s => bail!("检查新版本失败：GitHub 返回 {s}"),
    }
}

/// 下载附件到 dir 并按同名 `.sha256` 校验（发布里没有校验文件则拒绝）；返回文件路径
pub fn download(rel: &Release, asset: &Asset, dir: &Path, proxy: &ProxyPolicy) -> Result<PathBuf> {
    if !asset.url.starts_with("https://github.com/") {
        bail!("下载地址不在 github.com：{}", asset.url);
    }
    let sum = rel.checksum_of(asset).ok_or_else(|| anyhow!("发布里没有 {} 的校验文件，不安装", asset.name))?;
    let expected = get(&sum.url, "application/octet-stream", 30_000, proxy)?;
    if expected.status != 200 {
        bail!("下载校验文件失败：HTTP {}", expected.status);
    }
    let data = get(&asset.url, "application/octet-stream", 600_000, proxy)?;
    if data.status != 200 {
        bail!("下载 {} 失败：HTTP {}", asset.name, data.status);
    }
    verify_sha256(&data.body, &expected.body_text(), &asset.name)?;
    std::fs::create_dir_all(dir)?;
    let path = dir.join(&asset.name);
    std::fs::write(&path, &data.body).with_context(|| format!("写入 {} 失败", path.display()))?;
    Ok(path)
}

/// 校验文件的格式：`<64 位十六进制>  文件名`（sha256sum / shasum 的输出）
pub fn verify_sha256(data: &[u8], checksum_text: &str, name: &str) -> Result<()> {
    let want = checksum_text.split_whitespace().next().unwrap_or_default().to_ascii_lowercase();
    if want.len() != 64 || !want.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("{name} 的校验文件格式不对");
    }
    let got: String = Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect();
    if got != want {
        bail!("{name} 校验失败（下载损坏或被篡改）：期望 {want}，实际 {got}");
    }
    Ok(())
}

/// 当前可执行文件在 .app 里时不能就地替换（会破坏签名）：命令行升级只用于独立安装的 requestbaby
pub fn exe_in_app_bundle(exe: &Path) -> bool {
    exe.to_string_lossy().contains(".app/Contents/")
}

/// 用 tar.gz 里的 requestbaby 替换当前可执行文件；返回被替换的路径
pub fn install_cli(archive: &Path, expect_version: &str) -> Result<PathBuf> {
    let exe = std::env::current_exe().context("找不到当前可执行文件")?.canonicalize()?;
    if exe_in_app_bundle(&exe) {
        bail!("当前的 requestbaby 在 requestbaby.app 里：用 App 的「检查更新」或重新安装 DMG 更新，命令行会随 App 一起更新");
    }
    let dir = exe.parent().ok_or_else(|| anyhow!("可执行文件没有所在目录"))?;
    let tmp = std::env::temp_dir().join(format!("requestbaby-upgrade-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    let result = (|| -> Result<PathBuf> {
        let st = std::process::Command::new("tar").arg("-xzf").arg(archive).arg("-C").arg(&tmp).status().context("运行 tar 解压失败")?;
        if !st.success() {
            bail!("解压 {} 失败", archive.display());
        }
        let new_bin = tmp.join("requestbaby");
        if !new_bin.is_file() {
            bail!("安装包里没有 requestbaby");
        }
        // 先确认新程序能跑、版本对，再替换
        let out = std::process::Command::new(&new_bin).arg("--version").output().context("新版本无法运行")?;
        let shown = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !shown.contains(expect_version) {
            bail!("安装包里的版本是「{shown}」，不是 {expect_version}");
        }
        let staged = dir.join(".requestbaby.upgrade");
        std::fs::copy(&new_bin, &staged).with_context(|| format!("无法写入 {}：没有权限？用 sudo 重试，或换成你有权限的安装位置", dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
        }
        std::fs::rename(&staged, &exe).with_context(|| format!("替换 {} 失败", exe.display()))?;
        Ok(exe.clone())
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_ordering() {
        assert!(is_newer("0.4.0", "0.3.9"));
        assert!(is_newer("v1.0.0", "0.99.99"));
        assert!(is_newer("0.10.0", "0.9.0"), "按数字比较而不是按字符串");
        assert!(!is_newer("0.3.0", "0.3.0"));
        assert!(!is_newer("0.2.9", "0.3.0"));
        assert!(is_newer("0.4.0", "0.4.0-rc.1"), "正式版高于同号预发布");
        assert!(!is_newer("0.4.0-rc.1", "0.4.0"));
        assert!(!is_newer("nightly", "0.3.0"));
    }

    #[test]
    fn parses_github_release_and_picks_assets() {
        let json = r#"{"tag_name":"v0.4.0","html_url":"https://github.com/o/r/releases/tag/v0.4.0","body":" 新增 X \n","assets":[
            {"name":"requestbaby-aarch64-apple-darwin.tar.gz","browser_download_url":"https://github.com/o/r/releases/download/v0.4.0/requestbaby-aarch64-apple-darwin.tar.gz","size":10},
            {"name":"requestbaby-aarch64-apple-darwin.tar.gz.sha256","browser_download_url":"https://github.com/o/r/releases/download/v0.4.0/requestbaby-aarch64-apple-darwin.tar.gz.sha256","size":90},
            {"name":"requestbaby-0.4.0.dmg","browser_download_url":"https://github.com/o/r/releases/download/v0.4.0/requestbaby-0.4.0.dmg","size":30}]}"#;
        let r = parse_release(json).unwrap();
        assert_eq!((r.version.as_str(), r.tag.as_str(), r.notes.as_str()), ("0.4.0", "v0.4.0", "新增 X"));
        assert_eq!(r.dmg_asset().unwrap().name, "requestbaby-0.4.0.dmg");
        if target() == Some("aarch64-apple-darwin") {
            let a = r.cli_asset().unwrap();
            assert_eq!(r.checksum_of(a).unwrap().name, "requestbaby-aarch64-apple-darwin.tar.gz.sha256");
        }
        assert!(r.checksum_of(r.dmg_asset().unwrap()).is_none(), "没有校验文件的附件不安装");
        assert!(parse_release("{}").is_err());
    }

    #[test]
    fn checksum_detects_corruption() {
        let good = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9  x.tar.gz\n";
        assert!(verify_sha256(b"hello world", good, "x").is_ok());
        assert!(verify_sha256(b"hello w0rld", good, "x").unwrap_err().to_string().contains("校验失败"));
        assert!(verify_sha256(b"hello world", "not a hash", "x").is_err());
    }

    #[test]
    fn app_bundle_is_not_replaced_in_place() {
        assert!(exe_in_app_bundle(Path::new("/Applications/requestbaby.app/Contents/MacOS/requestbaby")));
        assert!(!exe_in_app_bundle(Path::new("/usr/local/bin/requestbaby")));
    }
}
