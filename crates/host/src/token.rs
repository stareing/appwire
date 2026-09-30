//! 本地访问令牌（`~/.app-mcp/token`）：首次使用时生成 32 字节随机数（64 位十六进制），
//! Unix 上权限 0600（Windows 上位于用户目录，继承用户专属 ACL）。

use std::io::Write;
use std::path::Path;

use anyhow::Context;
use rand::RngCore;

/// 读取令牌；文件不存在或为空时生成并写入。
pub fn load_or_create(path: &Path) -> anyhow::Result<String> {
    match std::fs::read_to_string(path) {
        Ok(t) if !t.trim().is_empty() => return Ok(t.trim().to_owned()),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(anyhow::anyhow!("读取令牌 {} 失败：{e}", path.display())),
    }
    write_new(path)
}

/// 生成新令牌并覆盖写入。
pub fn write_new(path: &Path) -> anyhow::Result<String> {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建目录 {} 失败", parent.display()))?;
    }
    // 先删除旧文件，确保新文件以 0600 创建（已有文件的权限不会被 mode 改变）。
    let _ = std::fs::remove_file(path);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("创建令牌文件 {} 失败", path.display()))?;
    file.write_all(token.as_bytes())
        .and_then(|_| file.write_all(b"\n"))
        .with_context(|| format!("写入令牌文件 {} 失败", path.display()))?;
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_then_reuse() {
        let dir = std::env::temp_dir().join(format!("app-mcp-token-{}", std::process::id()));
        let path = dir.join("token");
        let t1 = load_or_create(&path).unwrap();
        assert_eq!(t1.len(), 64);
        assert!(t1.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(load_or_create(&path).unwrap(), t1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let t2 = write_new(&path).unwrap();
        assert_ne!(t1, t2);
        assert_eq!(load_or_create(&path).unwrap(), t2);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
