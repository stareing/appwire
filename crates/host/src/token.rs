//! 本地访问令牌（`~/.app-mcp/token`）：首次使用时生成 32 字节随机数（64 位十六进制），
//! Unix 上权限 0600（Windows 上位于用户目录，继承用户专属 ACL）。Agent 令牌文件（`agents.json`）共用生成与写入。

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
    let token = generate();
    write_private(path, &format!("{token}\n"))?;
    Ok(token)
}

/// 生成一个令牌：32 字节随机数的十六进制（64 位）。本机令牌与 Agent 令牌（`agents.json`）共用。
pub fn generate() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 写入只有本用户可读的文件（含令牌）：先写同目录临时文件（Unix 0600）再替换，不留下半写或权限过宽的文件。
///
/// @security 临时文件以 `create_new` 创建（已有同名残留先删除），权限在创建时即为 0600，不经过默认权限的窗口；
/// Windows 上位于用户目录，继承用户专属 ACL。
pub fn write_private(path: &Path, contents: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建目录 {} 失败", parent.display()))?;
    }
    let mut tmp_name = path.file_name().unwrap_or_default().to_owned();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    let _ = std::fs::remove_file(&tmp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&tmp)
        .with_context(|| format!("创建 {} 失败", tmp.display()))?;
    file.write_all(contents.as_bytes())
        .and_then(|()| file.sync_all())
        .with_context(|| format!("写入 {} 失败", tmp.display()))?;
    drop(file);
    std::fs::rename(&tmp, path).with_context(|| format!("替换 {} 失败", path.display()))
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
