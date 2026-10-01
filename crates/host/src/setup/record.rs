//! `<home>/setup.json`：setup 实际做过的改动清单，`uninstall` 只按它撤销。
//!
//! @invariant 只记录本程序写入成功并校验通过的改动；`uninstall` 撤销成功的条目从清单中删除，失败的保留以便重试。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::agents::AgentId;
use super::files;

/// 清单文件名（位于配置目录）。
pub const SETUP_FILE: &str = "setup.json";
/// 清单格式版本。
pub const RECORD_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupRecord {
    pub version: u32,
    /// 复制到稳定位置的二进制（从 npx / uvx 等包管理器目录运行时）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary: Option<BinaryRecord>,
    /// setup 安装了登录自启服务。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<ServiceRecord>,
    #[serde(default)]
    pub agents: Vec<AgentRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinaryRecord {
    pub dir: PathBuf,
    pub files: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceRecord {
    pub location: String,
    pub exe: PathBuf,
}

/// 写入某个 Agent 配置的一条记录。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRecord {
    pub agent: AgentId,
    /// MCP 服务器条目名。
    pub name: String,
    /// 写入的 URL。
    pub url: String,
    /// 被修改的配置文件（经 Agent 自己的命令写入时为该命令修改的文件）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
    /// 第一次写入前的备份；`None` 且 `file` 有值 = 写入前文件不存在。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<PathBuf>,
    /// 最近一次写入后的文件指纹：卸载时文件仍是这个指纹才整文件恢复备份，否则只删本条目。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub written_hash: Option<String>,
    /// `--force` 覆盖掉的原条目（说明文字），卸载时提示用户备份位置。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced: Option<String>,
}

pub fn path(home: &Path) -> PathBuf {
    home.join(SETUP_FILE)
}

impl SetupRecord {
    pub fn load(home: &Path) -> anyhow::Result<Option<Self>> {
        let Some(v) = files::read_json(&path(home))? else {
            return Ok(None);
        };
        let r: Self = serde_json::from_value(v)
            .map_err(|e| anyhow::anyhow!("解析 {} 失败：{e}", path(home).display()))?;
        if r.version > RECORD_VERSION {
            anyhow::bail!(
                "{} 的版本 {} 高于本程序支持的 {RECORD_VERSION}：请用更新版本的 app-mcp-host 运行",
                path(home).display(),
                r.version
            );
        }
        Ok(Some(r))
    }

    /// 写入；清单为空（没有任何改动）时删除文件。
    pub fn save(&self, home: &Path) -> anyhow::Result<()> {
        if self.is_empty() {
            return files::restore(&path(home), None);
        }
        let v = serde_json::to_value(Self { version: RECORD_VERSION, ..self.clone() })?;
        files::write_json(&path(home), &v)
    }

    pub fn is_empty(&self) -> bool {
        self.binary.is_none() && self.service.is_none() && self.agents.is_empty()
    }

    pub fn agent(&self, id: AgentId) -> Option<&AgentRecord> {
        self.agents.iter().find(|a| a.agent == id)
    }

    /// 新增或替换同一 Agent 的记录（保留最早的备份：它代表 setup 之前的原状）。
    pub fn upsert_agent(&mut self, mut rec: AgentRecord) {
        match self.agents.iter_mut().find(|a| a.agent == rec.agent) {
            Some(old) => {
                if old.file == rec.file {
                    rec.backup = old.backup.take();
                    rec.replaced = rec.replaced.or(old.replaced.take());
                }
                *old = rec;
            }
            None => self.agents.push(rec),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(url: &str, backup: Option<&str>) -> AgentRecord {
        AgentRecord {
            agent: AgentId::Cursor,
            name: "app-mcp".into(),
            url: url.into(),
            file: Some("/h/.cursor/mcp.json".into()),
            backup: backup.map(PathBuf::from),
            written_hash: Some("h".into()),
            replaced: None,
        }
    }

    #[test]
    fn upsert_keeps_first_backup() {
        let mut r = SetupRecord::default();
        r.upsert_agent(rec("http://a/mcp", Some("/b/1.bak")));
        r.upsert_agent(rec("http://b/mcp", Some("/b/2.bak")));
        assert_eq!(r.agents.len(), 1);
        assert_eq!(r.agents[0].url, "http://b/mcp");
        assert_eq!(r.agents[0].backup.as_deref(), Some(Path::new("/b/1.bak")));
    }

    #[test]
    fn save_load_and_empty_removes_file() {
        let n: u64 = rand::random();
        let home = std::env::temp_dir().join(format!("appwire-record-{n:x}"));
        let mut r = SetupRecord::default();
        r.upsert_agent(rec("http://a/mcp", None));
        r.save(&home).unwrap();
        let back = SetupRecord::load(&home).unwrap().unwrap();
        assert_eq!(back.version, RECORD_VERSION);
        assert_eq!(back.agents, r.agents);
        SetupRecord::default().save(&home).unwrap();
        assert!(SetupRecord::load(&home).unwrap().is_none());
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn rejects_newer_version() {
        let n: u64 = rand::random();
        let home = std::env::temp_dir().join(format!("appwire-record-v-{n:x}"));
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(path(&home), r#"{"version":99,"agents":[]}"#).unwrap();
        assert!(SetupRecord::load(&home).is_err());
        let _ = std::fs::remove_dir_all(home);
    }
}
