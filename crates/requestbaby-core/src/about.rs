//! 关于：官网、源码、反馈、更新日志的地址。桌面端的「关于 / 帮助」菜单与命令行 `requestbaby about` 共用这一份。

/// 官网（下载、截图、常见问题）
pub const WEBSITE: &str = "https://apps.zhizuzhefu.com/requestbaby/";
/// 源码与文档
pub const SOURCE: &str = "https://github.com/zhizuzhefu/requestbaby";
/// 反馈问题与建议
pub const ISSUES: &str = "https://github.com/zhizuzhefu/requestbaby/issues";
/// 更新日志
pub const CHANGELOG: &str = "https://github.com/zhizuzhefu/requestbaby/blob/main/CHANGELOG.md";
/// 命令行文档所在（与 `requestbaby guide` 内容一致）
pub const DOCS: &str = "https://github.com/zhizuzhefu/requestbaby/tree/main/docs/guide";

/// 命令行 `requestbaby about` 的输出
pub fn text() -> String {
    format!(
        "requestbaby {version} — AI 交付的独立验收层，辅助人对 AI 交付的成果验收。\n\
         官网：{WEBSITE}\n\
         源码：{SOURCE}\n\
         文档：{DOCS}\n\
         更新日志：{CHANGELOG}\n\
         反馈：{ISSUES}\n\
         许可：MIT",
        version = crate::update::CURRENT,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_lists_every_entry() {
        let t = text();
        for url in [WEBSITE, SOURCE, ISSUES, CHANGELOG, DOCS] {
            assert!(t.contains(url), "{url}");
        }
        assert!(t.contains(crate::update::CURRENT));
    }
}
