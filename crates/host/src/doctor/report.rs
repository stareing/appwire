//! doctor 的结果模型：检查级别、单项检查与诊断报告（含文本渲染）。

use app_mcp_protocol::ConnectionErrorCode;
use serde::Serialize;
use serde_json::Value;

/// 检查结果的级别。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Ok,
    Info,
    Warn,
    Error,
    Skip,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Level::Ok => "正常",
            Level::Info => "信息",
            Level::Warn => "注意",
            Level::Error => "错误",
            Level::Skip => "跳过",
        }
    }
}

/// 一项检查。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub id: &'static str,
    pub title: &'static str,
    pub status: Level,
    /// 结论。
    pub summary: String,
    /// 修复建议。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// 相关的错误码（spec/protocol.md 10.1）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<&'static str>,
    pub details: Value,
}

impl Check {
    pub(super) fn new(id: &'static str, title: &'static str, status: Level, summary: impl Into<String>) -> Self {
        Self { id, title, status, summary: summary.into(), hint: None, code: None, details: Value::Null }
    }
    pub(super) fn hint(mut self, h: impl Into<String>) -> Self {
        self.hint = Some(h.into());
        self
    }
    pub(super) fn code(mut self, c: ConnectionErrorCode) -> Self {
        self.code = Some(c.as_str());
        if self.hint.is_none() {
            self.hint = Some(c.hint().to_owned());
        }
        self
    }
    pub(super) fn details(mut self, d: Value) -> Self {
        self.details = d;
        self
    }
}

/// 诊断报告。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// 本程序版本。
    pub version: &'static str,
    pub home: String,
    pub checks: Vec<Check>,
}

impl Report {
    /// 有 `error` 级别的检查。
    pub fn has_errors(&self) -> bool {
        self.checks.iter().any(|c| c.status == Level::Error)
    }

    /// 人类可读的输出。
    pub fn render(&self) -> String {
        let mut out = format!("app-mcp-host doctor {}（配置目录 {}）\n", self.version, self.home);
        for c in &self.checks {
            out.push_str(&format!("\n[{}] {}\n  结论：{}\n", c.status.label(), c.title, c.summary));
            if let Some(h) = &c.hint {
                out.push_str(&format!("  建议：{h}\n"));
            }
            if let Some(code) = c.code {
                out.push_str(&format!("  错误码：{code}\n"));
            }
        }
        let count = |l: Level| self.checks.iter().filter(|c| c.status == l).count();
        out.push_str(&format!(
            "\n共 {} 项：错误 {}，注意 {}，正常 {}\n",
            self.checks.len(),
            count(Level::Error),
            count(Level::Warn),
            count(Level::Ok)
        ));
        out
    }
}
