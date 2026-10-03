//! 静态能力清单 `app-mcp.json`（规范见 `spec/manifest.md`）。
//!
//! - [`parse`]：从 JSON 文本解析 [`Manifest`]（只做结构解析，不做语义校验）。
//! - [`Manifest::validate`]：按规范第 3 节校验，返回错误与警告列表。
//! - [`load_file`] / [`load_dir`]：读取文件并解析 + 校验；目录中读取所有 `*.json`。
//!
//! 未知的 launch `type` 会原样保留（[`LaunchEntry::Other`]），校验时只给出警告。
//! `wake`（唤醒描述，spec/lifecycle.md 第 5 节）同理：未知 `kind` 保留在 [`WakeEntry::Other`]。

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

pub use app_mcp_protocol::{Activation, AppOverview, ResourceInfo, ToolInfo};
use app_mcp_protocol::{
    OVERVIEW_BODY_MAX_CHARS, OVERVIEW_SUMMARY_MAX_CHARS, is_valid_app_id, is_valid_name,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 当前支持的清单版本。
pub const MANIFEST_VERSION: u64 = 1;

/// 保留的 appId：被 Host 内置工具或未来的系统能力占用。
pub const RESERVED_APP_IDS: &[&str] = &["apps", "os", "ax", "host"];

/// 判断 appId 是否为保留名。
pub fn is_reserved_app_id(id: &str) -> bool {
    RESERVED_APP_IDS.contains(&id)
}

mod types;
mod validate;

pub use types::*;
pub use validate::*;

/// 加载失败。
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("读取清单 {path} 失败：{source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("解析清单{} 失败：{source}", display_path(.path))]
    Parse {
        path: Option<PathBuf>,
        #[source]
        source: serde_json::Error,
    },
    #[error("清单{} 校验失败：{}", display_path(.path), join_issues(.errors))]
    Invalid {
        path: Option<PathBuf>,
        errors: Vec<Issue>,
    },
}

fn display_path(path: &Option<PathBuf>) -> String {
    path.as_ref()
        .map(|p| format!(" {}", p.display()))
        .unwrap_or_default()
}

fn join_issues(issues: &[Issue]) -> String {
    issues
        .iter()
        .map(Issue::to_string)
        .collect::<Vec<_>>()
        .join("；")
}

/// 解析清单文本（只做结构解析）。
pub fn parse(text: &str) -> Result<Manifest, ManifestError> {
    serde_json::from_str(text).map_err(|source| ManifestError::Parse { path: None, source })
}

/// 解析并校验成功的清单。
#[derive(Clone, Debug, PartialEq)]
pub struct LoadedManifest {
    /// 来源文件；从文本加载时为 `None`。
    pub path: Option<PathBuf>,
    pub manifest: Manifest,
    pub warnings: Vec<Issue>,
}

/// 解析并校验清单文本。
pub fn load_str(text: &str) -> Result<LoadedManifest, ManifestError> {
    let manifest = parse(text)?;
    let validation = manifest.validate();
    if !validation.is_ok() {
        return Err(ManifestError::Invalid {
            path: None,
            errors: validation.errors,
        });
    }
    Ok(LoadedManifest {
        path: None,
        manifest,
        warnings: validation.warnings,
    })
}

/// 读取、解析并校验单个清单文件。
pub fn load_file(path: impl AsRef<Path>) -> Result<LoadedManifest, ManifestError> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path).map_err(|source| ManifestError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    load_str(&text)
        .map(|mut loaded| {
            loaded.path = Some(path.to_path_buf());
            loaded
        })
        .map_err(|e| match e {
            ManifestError::Parse { source, .. } => ManifestError::Parse {
                path: Some(path.to_path_buf()),
                source,
            },
            ManifestError::Invalid { errors, .. } => ManifestError::Invalid {
                path: Some(path.to_path_buf()),
                errors,
            },
            other => other,
        })
}

/// 读取目录下所有 `*.json`（不递归），按文件名排序后逐个加载。
///
/// 目录本身无法读取时返回 `Err`；单个文件的失败放在结果列表中，不影响其他文件。
pub fn load_dir(
    dir: impl AsRef<Path>,
) -> std::io::Result<Vec<Result<LoadedManifest, ManifestError>>> {
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(dir.as_ref())? {
        let path = entry?.path();
        let is_json = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("json"));
        if is_json && path.is_file() {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths.into_iter().map(load_file).collect())
}

#[cfg(test)]
mod tests;
