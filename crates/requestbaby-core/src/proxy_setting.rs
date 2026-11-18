//! requestbaby.json 的代理设置：CLI `requestbaby proxy` 与桌面端项目设置共用同一读写。
//!
//! 三种状态：不写 `proxy`（跟随系统：环境变量 → 系统代理设置）/ `enabled: true`（全部经代理）/ `enabled: false`（全部直连）。
//! 禁用时保留 url 与 noProxy，再启用不用重填。
use crate::config::{load_strict, Project, ProxyConfig};
use crate::runner::discover;
use anyhow::{anyhow, bail, Context, Result};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 跟随系统：环境变量（HTTPS_PROXY / ALL_PROXY …）→ 系统代理设置 → 直连
    System,
    /// 使用代理
    On,
    /// 不使用代理：全部直连
    Off,
}

impl Mode {
    pub fn of(cfg: Option<&ProxyConfig>) -> Self {
        match cfg {
            None => Self::System,
            Some(c) if c.enabled => Self::On,
            Some(_) => Self::Off,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "跟随系统",
            Self::On => "使用代理",
            Self::Off => "不使用代理（直连）",
        }
    }
}

/// 检查代理地址（含 {{变量}} 的到运行时再检查）
pub fn check_url(url: &str) -> Result<()> {
    if url.contains("{{") {
        return Ok(());
    }
    crate::engine::proxy::Proxy::parse(url).map(|_| ())
}

/// 逗号 / 空白分隔的直连主机列表
pub fn parse_no_proxy(text: &str) -> Vec<String> {
    text.split([',', '，', ' ', '\n']).map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

/// 按模式组装设置；Env = 不写 proxy
pub fn build(mode: Mode, url: &str, no_proxy: Vec<String>) -> Result<Option<ProxyConfig>> {
    let url = url.trim().to_string();
    match mode {
        Mode::System => Ok(None),
        Mode::On if url.is_empty() => bail!("使用代理需要填写代理地址（socks5://主机:端口）"),
        _ => {
            if !url.is_empty() {
                check_url(&url)?;
            }
            Ok(Some(ProxyConfig { enabled: mode == Mode::On, url, no_proxy }))
        }
    }
}

pub fn read(root: &Path) -> Result<Option<ProxyConfig>> {
    let pf = discover::project_file(root).ok_or_else(|| anyhow!("没有 requestbaby.json"))?;
    Ok(load_strict::<Project>(&pf)?.proxy)
}

/// 写入 requestbaby.json（None = 删除 proxy，跟随系统）；其余字段原样保留
pub fn write(root: &Path, cfg: Option<&ProxyConfig>) -> Result<()> {
    let pf = discover::project_file(root).ok_or_else(|| anyhow!("没有 requestbaby.json"))?;
    let text = std::fs::read_to_string(&pf)?;
    let mut doc: serde_json::Value = serde_json::from_str(&text).with_context(|| format!("{} 不是合法 JSON", pf.display()))?;
    let o = doc.as_object_mut().ok_or_else(|| anyhow!("requestbaby.json 不是对象"))?;
    match cfg {
        Some(c) => o.insert("proxy".into(), serde_json::to_value(c)?),
        None => o.remove("proxy"),
    };
    crate::fsx::write(&pf, crate::fmt::canonical_project_text(&doc)).with_context(|| format!("写入失败: {}", pf.display()))
}
