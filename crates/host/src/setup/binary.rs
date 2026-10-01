//! 二进制的稳定位置：从包管理器目录（npx / uvx 缓存、全局 node_modules、site-packages）运行时，把自身复制到
//! `<home>/bin/` 再注册自启。
//!
//! @why 登录自启项记录可执行文件的绝对路径；包管理器目录会被清理或在升级 / 卸载时替换（packaging/README.md「已知限制」），
//! 自启项随之失效。选 `<home>/bin/` 而不是 `~/.local/bin` 等：与配置、令牌、日志同在每用户的配置目录（无需管理员、
//! 不依赖 PATH——服务用绝对路径），`uninstall --purge` 可整体删除，且不会覆盖用户在 PATH 中自行管理的同名程序；
//! 测试用临时 `--home` 即可隔离。

use std::path::{Component, Path, PathBuf};

use anyhow::Context;

use super::files;
use crate::service;

/// `<home>/bin`。
pub const BIN_DIR: &str = "bin";

/// 包管理器管理的目录名：路径中出现任一即认为位置不稳定。
const PACKAGE_MANAGER_DIRS: &[&str] = &["node_modules", "site-packages", "_npx"];

/// 是否位于包管理器目录中。
pub fn is_package_managed(exe: &Path) -> bool {
    exe.components().any(|c| match c {
        Component::Normal(n) => PACKAGE_MANAGER_DIRS.iter().any(|d| n.eq_ignore_ascii_case(d)),
        _ => false,
    })
}

/// 需要随主程序一起复制的文件名（Windows 上另有无窗口版，见 [`service::WINDOWS_BACKGROUND_EXE`]）。
pub fn companion_files(exe: &Path) -> Vec<PathBuf> {
    let mut v = vec![exe.to_path_buf()];
    if cfg!(windows) {
        v.push(service::background_exe_for(exe));
    }
    v
}

/// 二进制安排。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BinaryPlan {
    /// 当前运行的主程序。
    pub current: PathBuf,
    /// 复制到的目录；`None` = 原地使用。
    pub target_dir: Option<PathBuf>,
}

impl BinaryPlan {
    pub fn new(current: &Path, home_dir: &Path) -> Self {
        let bin = home_dir.join(BIN_DIR);
        // 已在 <home>/bin 中运行（如 setup 的自启项再次执行 setup）时原地使用。
        let in_bin = current.parent() == Some(bin.as_path());
        Self {
            current: current.to_path_buf(),
            target_dir: (is_package_managed(current) && !in_bin).then_some(bin),
        }
    }

    /// 服务使用的主程序路径（复制后为目标目录中的同名文件）。
    pub fn stable_exe(&self) -> PathBuf {
        match (&self.target_dir, self.current.file_name()) {
            (Some(dir), Some(name)) => dir.join(name),
            _ => self.current.clone(),
        }
    }

    /// 执行复制；返回 (目标文件列表, 是否有文件内容变化)。原地使用时返回空列表。
    pub fn install(&self) -> anyhow::Result<(Vec<PathBuf>, bool)> {
        let Some(dir) = &self.target_dir else {
            return Ok((Vec::new(), false));
        };
        std::fs::create_dir_all(dir).with_context(|| format!("创建目录 {} 失败", dir.display()))?;
        let mut targets = Vec::new();
        let mut changed = false;
        for src in companion_files(&self.current) {
            let Some(name) = src.file_name() else { continue };
            let dest = dir.join(name);
            changed |= copy_if_changed(&src, &dest)?;
            targets.push(dest);
        }
        Ok((targets, changed))
    }
}

/// 内容不同才复制（临时文件 + rename，保留可执行权限）；返回是否复制了。
fn copy_if_changed(src: &Path, dest: &Path) -> anyhow::Result<bool> {
    if files::fingerprint(dest)? == files::fingerprint(src)? {
        return Ok(false);
    }
    let tmp = dest.with_extension(format!("appwire-{}.tmp", std::process::id()));
    std::fs::copy(src, &tmp).with_context(|| format!("复制 {} 到 {} 失败", src.display(), tmp.display()))?;
    if let Err(e) = replace(&tmp, dest) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(true)
}

/// rename 覆盖目标。
///
/// @compat Windows 上运行中的 exe 不能被覆盖但可以改名：先把旧文件改名为 `.old`（下次复制时清理），再放入新文件。
fn replace(tmp: &Path, dest: &Path) -> anyhow::Result<()> {
    match std::fs::rename(tmp, dest) {
        Ok(()) => Ok(()),
        Err(_) if cfg!(windows) && dest.exists() => {
            let old = dest.with_extension("old");
            let _ = std::fs::remove_file(&old);
            std::fs::rename(dest, &old).with_context(|| format!("改名 {} 失败", dest.display()))?;
            std::fs::rename(tmp, dest).with_context(|| format!("放入 {} 失败", dest.display()))
        }
        Err(e) => Err(e).with_context(|| format!("放入 {} 失败", dest.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_manager_paths() {
        assert!(is_package_managed(Path::new("/home/u/.npm/_npx/ab12/node_modules/appwire-cli-linux-x64/bin/app-mcp-host")));
        assert!(is_package_managed(Path::new("/usr/lib/node_modules/appwire-cli-linux-x64/bin/app-mcp-host")));
        assert!(is_package_managed(Path::new(
            "/home/u/.cache/uv/archive-v0/x/lib/python3.13/site-packages/appwire_cli/bin/app-mcp-host"
        )));
        // Windows 路径（大小写不敏感）只在 Windows 上按 `\` 分段
        #[cfg(windows)]
        assert!(is_package_managed(Path::new(r"C:\Users\u\AppData\Local\npm-cache\_npx\1\Node_Modules\x\bin\app-mcp-host.exe")));
        assert!(!is_package_managed(Path::new("/home/u/.cargo/bin/app-mcp-host")));
        assert!(!is_package_managed(Path::new("/repo/target/debug/app-mcp-host")));
        assert!(!is_package_managed(Path::new("/opt/my_node_modules_tool/app-mcp-host")));
    }

    #[test]
    fn plan_targets() {
        let home = Path::new("/h/.app-mcp");
        let p = BinaryPlan::new(Path::new("/x/node_modules/p/bin/app-mcp-host"), home);
        assert_eq!(p.stable_exe(), home.join("bin").join("app-mcp-host"));
        let p = BinaryPlan::new(Path::new("/h/.cargo/bin/app-mcp-host"), home);
        assert_eq!(p.target_dir, None);
        assert_eq!(p.stable_exe(), Path::new("/h/.cargo/bin/app-mcp-host"));
    }

    #[test]
    fn copies_once_then_unchanged() {
        let n: u64 = rand::random();
        let root = std::env::temp_dir().join(format!("appwire-bin-{n:x}"));
        let src_dir = root.join("node_modules").join("pkg").join("bin");
        std::fs::create_dir_all(&src_dir).unwrap();
        let exe = src_dir.join("app-mcp-host");
        std::fs::write(&exe, b"v1").unwrap();
        if cfg!(windows) {
            std::fs::write(service::background_exe_for(&exe), b"w1").unwrap();
        }
        let home = root.join("home");
        let plan = BinaryPlan::new(&exe, &home);
        let (files, changed) = plan.install().unwrap();
        assert!(changed);
        assert_eq!(std::fs::read(&files[0]).unwrap(), b"v1");
        assert!(!plan.install().unwrap().1, "内容相同不复制");
        std::fs::write(&exe, b"v2").unwrap();
        assert!(plan.install().unwrap().1);
        assert_eq!(std::fs::read(plan.stable_exe()).unwrap(), b"v2");
        let _ = std::fs::remove_dir_all(root);
    }
}
