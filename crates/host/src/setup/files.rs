//! setup 用到的文件操作：内容指纹、备份、原子写入、严格 JSON 读取。
//!
//! @invariant 只写调用方给出的路径；写入先落临时文件再 rename（同目录），中途失败不留半截文件。

use std::path::{Path, PathBuf};

use anyhow::Context;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// 文件内容的 sha256（十六进制）；文件不存在时为 `None`。
pub fn fingerprint(path: &Path) -> anyhow::Result<Option<String>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(hex_sha256(&bytes))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("读取 {} 失败", path.display())),
    }
}

pub fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// 把 `path` 复制到 `dir/<tag>-<毫秒时间戳>.bak`；源文件不存在时返回 `None`（恢复 = 删除该文件）。
pub fn backup(path: &Path, dir: &Path, tag: &str) -> anyhow::Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    std::fs::create_dir_all(dir).with_context(|| format!("创建目录 {} 失败", dir.display()))?;
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let mut dest = dir.join(format!("{tag}-{ms}.bak"));
    let mut n = 1;
    while dest.exists() {
        dest = dir.join(format!("{tag}-{ms}-{n}.bak"));
        n += 1;
    }
    std::fs::copy(path, &dest)
        .with_context(|| format!("备份 {} 到 {} 失败", path.display(), dest.display()))?;
    Ok(Some(dest))
}

/// 恢复到写入前：有备份则逐字节复制回去，没有（原本不存在）则删除文件。
pub fn restore(path: &Path, backup: Option<&Path>) -> anyhow::Result<()> {
    match backup {
        Some(b) => {
            let bytes = std::fs::read(b).with_context(|| format!("读取备份 {} 失败", b.display()))?;
            write_atomic(path, &bytes)
        }
        None => match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("删除 {} 失败", path.display())),
        },
    }
}

/// 同目录临时文件 + rename；必要时创建父目录。
pub fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).with_context(|| format!("创建目录 {} 失败", parent.display()))?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = parent.join(format!(".{name}.appwire-{}.tmp", std::process::id()));
    let result = std::fs::write(&tmp, bytes)
        .and_then(|()| copy_permissions(path, &tmp))
        .and_then(|()| std::fs::rename(&tmp, path));
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("写入 {} 失败", path.display()));
    }
    Ok(())
}

/// 保留原文件的权限（如 0600 的配置文件）；原文件不存在时不处理。
fn copy_permissions(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::metadata(from) {
        Ok(m) => std::fs::set_permissions(to, m.permissions()),
        Err(_) => Ok(()),
    }
}

/// 严格 JSON 读取：不存在 → `Ok(None)`；含注释等非标准 JSON → 错误（不猜测、不改写）。
pub fn read_json(path: &Path) -> anyhow::Result<Option<Value>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("读取 {} 失败", path.display())),
    };
    if text.trim().is_empty() {
        return Ok(Some(Value::Object(Default::default())));
    }
    serde_json::from_str(&text)
        .map(Some)
        .with_context(|| format!("{} 不是标准 JSON（可能含注释或尾逗号），不自动修改", path.display()))
}

/// 格式化（两空格缩进）后原子写入。
pub fn write_json(path: &Path, value: &Value) -> anyhow::Result<()> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    write_atomic(path, text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let n: u64 = rand::random();
        let d = std::env::temp_dir().join(format!("appwire-files-{tag}-{n:x}"));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn backup_and_restore_round_trip() {
        let d = temp("rt");
        let f = d.join("a.json");
        std::fs::write(&f, b"{\"x\":1}").unwrap();
        let before = fingerprint(&f).unwrap();
        let b = backup(&f, &d.join("bk"), "t").unwrap().unwrap();
        write_json(&f, &serde_json::json!({"y": 2})).unwrap();
        assert_ne!(fingerprint(&f).unwrap(), before);
        restore(&f, Some(&b)).unwrap();
        assert_eq!(fingerprint(&f).unwrap(), before);
        // 原本不存在：备份为 None，恢复 = 删除
        let g = d.join("new.json");
        assert!(backup(&g, &d.join("bk"), "t").unwrap().is_none());
        write_json(&g, &serde_json::json!({})).unwrap();
        restore(&g, None).unwrap();
        assert!(!g.exists());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn strict_json_only() {
        let d = temp("json");
        let f = d.join("c.json");
        assert!(read_json(&f).unwrap().is_none());
        std::fs::write(&f, "{ // 注释\n \"a\": 1 }").unwrap();
        assert!(read_json(&f).is_err());
        std::fs::write(&f, "  \n").unwrap();
        assert_eq!(read_json(&f).unwrap(), Some(serde_json::json!({})));
        let _ = std::fs::remove_dir_all(d);
    }

    #[cfg(unix)]
    #[test]
    fn keeps_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let d = temp("perm");
        let f = d.join("p.json");
        std::fs::write(&f, "{}").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)).unwrap();
        write_json(&f, &serde_json::json!({"a": 1})).unwrap();
        assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(d);
    }
}
