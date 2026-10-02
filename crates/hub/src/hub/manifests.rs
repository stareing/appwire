//! 静态清单加载（spec/manifest.md 第 4 节）。

use std::path::{Path, PathBuf};

use app_mcp_manifest::Manifest;

/// 按 spec/manifest.md 第 4 节加载清单：先读目录（按文件名排序），再读 `--manifest` 文件。
/// 失败的清单记录错误后跳过。
pub fn load_manifests(files: &[PathBuf], dir: Option<&Path>, dir_required: bool) -> Vec<Manifest> {
    let mut out = Vec::new();
    let mut push =
        |r: Result<app_mcp_manifest::LoadedManifest, app_mcp_manifest::ManifestError>| match r {
            Ok(loaded) => {
                for w in &loaded.warnings {
                    tracing::warn!(app_id = %loaded.manifest.app_id, "清单警告：{w}");
                }
                tracing::info!(app_id = %loaded.manifest.app_id, path = ?loaded.path, "已加载清单");
                out.push(loaded.manifest);
            }
            Err(e) => tracing::error!("{e}"),
        };
    if let Some(dir) = dir {
        match app_mcp_manifest::load_dir(dir) {
            Ok(results) => results.into_iter().for_each(&mut push),
            Err(e) if !dir_required && e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!(dir = %dir.display(), "清单目录不存在，忽略");
            }
            Err(e) => tracing::error!(dir = %dir.display(), "读取清单目录失败：{e}"),
        }
    }
    for f in files {
        push(app_mcp_manifest::load_file(f));
    }
    out
}
