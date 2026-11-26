//! 原子写文件：先写同目录的临时文件再改名替换。
//!
//! 用例、配置、运行历史都可能同时被 CLI、桌面端、AI 读写：直接覆盖写到一半时崩溃 / 断电 / 磁盘满，
//! 文件会只剩半截；监听文件变化的一方也可能读到半个文件。改名在同一文件系统内是原子的，
//! 读的一方要么看到旧内容、要么看到完整的新内容。
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// 与 `std::fs::write` 同签名的原子版本；保留原文件权限，符号链接写到它指向的文件
pub fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    let path = resolve(path.as_ref());
    // /dev/stdout、命名管道这类特殊文件不能改名替换：直接写
    if std::fs::metadata(&path).is_ok_and(|m| !m.is_file()) {
        return std::fs::write(&path, contents);
    }
    let tmp = temp_path(&path);
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(contents.as_ref())?;
        f.sync_all()?;
        if let Ok(meta) = std::fs::metadata(&path) {
            std::fs::set_permissions(&tmp, meta.permissions())?;
        }
        std::fs::rename(&tmp, &path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn resolve(path: &Path) -> PathBuf {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()),
        _ => path.to_path_buf(),
    }
}

/// 同目录、点开头、扩展名不是 .json：用例发现与文件监听都会忽略它
fn temp_path(path: &Path) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(".{name}.{}-{seq}.rbtmp", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_content_keeps_permissions_and_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.json");
        write(&p, "{}").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        write(&p, "{\"a\": 1}").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{\"a\": 1}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(names, ["a.json"]);
        // 目录不存在：报错且不留临时文件
        assert!(write(dir.path().join("missing/b.json"), "x").is_err());
        // 特殊文件直接写
        #[cfg(unix)]
        write("/dev/null", "x").unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn writes_through_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.json");
        std::fs::write(&real, "old").unwrap();
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write(&link, "new").unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "new");
    }
}
