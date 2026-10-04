//! `validate <清单> [--against <旧清单>] [--json]`：静态清单校验与工具定义的兼容判定（spec/manifest.md 第 6 节）。
//!
//! 判定规则只在 `app_mcp_protocol::schema_compat` 实现；这里只负责加载、按工具名配对（顶层与页面内工具共用一个命名空间）与输出。

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::process::ExitCode;

use anyhow::bail;
use app_mcp_manifest::{LoadedManifest, Manifest, ToolInfo};
use app_mcp_protocol::schema_compat::{ChangeLevel, SchemaChange, ToolChanges, compare_tools};
use serde::Serialize;

use crate::cli::ValidateArgs;

/// 存在破坏性变更时的退出码（spec/manifest.md 第 6 节）。
pub const EXIT_BREAKING: u8 = 3;

/// 清单中一个工具的变化：页面内工具带页面名。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ManifestToolChanges {
    /// 所在页面（`pages[].name`）；顶层工具省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
    #[serde(flatten)]
    pub changes: ToolChanges,
}

/// @output 清单无效 → `Err`（退出码 1）；有破坏性变更 → [`EXIT_BREAKING`]；否则 0。
/// @side-effect 报告写 stdout，清单的校验警告写 stderr。
pub(crate) fn validate_cmd(args: &ValidateArgs) -> anyhow::Result<ExitCode> {
    let new = load(&args.manifest, "清单", true)?;
    let Some(against) = &args.against else {
        let m = &new.manifest;
        println!(
            "清单有效：{}（appId `{}`，{} 个顶层工具，{} 个页面）",
            args.manifest.display(),
            m.app_id,
            m.tools.len(),
            m.pages.len()
        );
        return Ok(ExitCode::SUCCESS);
    };
    let old = load(against, "旧清单", false)?;
    if old.manifest.app_id != new.manifest.app_id {
        bail!(
            "新旧清单的 appId 不同（`{}` / `{}`）：兼容判定只比较同一 App 的两个版本",
            new.manifest.app_id,
            old.manifest.app_id
        );
    }
    let entries = compare_manifests(&old.manifest, &new.manifest);
    if args.json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else {
        print!("{}", render(&new.manifest.app_id, &entries));
    }
    let breaking = entries.iter().any(|e| e.changes.has_breaking());
    Ok(if breaking { ExitCode::from(EXIT_BREAKING) } else { ExitCode::SUCCESS })
}

/// 读取并校验清单。`role` 用于错误信息（区分新旧清单）；`warn` 时校验警告写 stderr（旧清单的警告不再有意义，不打印）。
fn load(path: &Path, role: &str, warn: bool) -> anyhow::Result<LoadedManifest> {
    let loaded = app_mcp_manifest::load_file(path).map_err(|e| anyhow::anyhow!("{role}无效：{e}"))?;
    for w in loaded.warnings.iter().filter(|_| warn) {
        eprintln!("警告：{}：{w}", path.display());
    }
    Ok(loaded)
}

/// 顶层工具与页面内工具的变化：按工具名配对（清单内工具名唯一，与所在页面无关）；工具在顶层与页面之间或页面之间移动时
/// 记一条可能破坏（是否需要导航、是否作为静态工具列出随之变化），调用契约本身按 schema 比较。
///
/// @output 按工具名排序；`page` 为新清单中的所在页面（已删除的工具取旧清单中的）。
pub(crate) fn compare_manifests(old: &Manifest, new: &Manifest) -> Vec<ManifestToolChanges> {
    let all = |m: &Manifest| -> (Vec<ToolInfo>, BTreeMap<String, Option<String>>) {
        let top = m.tools.iter().map(|t| (t.clone(), None));
        let paged = m.pages.iter().flat_map(|p| p.tools.iter().map(move |t| (t.clone(), Some(p.name.clone()))));
        let (tools, pages): (Vec<_>, Vec<_>) = top.chain(paged).map(|(t, p)| (t.clone(), (t.name, p))).unzip();
        (tools, pages.into_iter().collect())
    };
    let ((old_tools, old_pages), (new_tools, new_pages)) = (all(old), all(new));
    let mut by_tool: BTreeMap<String, ToolChanges> =
        compare_tools(&old_tools, &new_tools).into_iter().map(|c| (c.tool.clone(), c)).collect();
    for (name, before) in &old_pages {
        let Some(after) = new_pages.get(name) else { continue };
        if before == after {
            continue;
        }
        let label = |p: &Option<String>| p.as_deref().map_or("顶层".to_owned(), |p| format!("页面 {p}"));
        let entry = by_tool
            .entry(name.clone())
            .or_insert_with(|| ToolChanges { tool: name.clone(), removed: false, changes: Vec::new() });
        entry.changes.insert(0, SchemaChange {
            level: ChangeLevel::Warning,
            path: String::new(),
            message: format!("所在位置由{}移到{}（导航与静态列出方式随之变化）", label(before), label(after)),
        });
    }
    by_tool
        .into_values()
        .map(|changes| {
            let page = new_pages.get(&changes.tool).or_else(|| old_pages.get(&changes.tool)).cloned().flatten();
            ManifestToolChanges { page, changes }
        })
        .collect()
}

/// 文本报告：按工具分组列出 breaking / warning，末尾一行结论。
fn render(app_id: &str, entries: &[ManifestToolChanges]) -> String {
    let count = |level| entries.iter().flat_map(|e| &e.changes.changes).filter(|c| c.level == level).count();
    let (breaking, warning) = (count(ChangeLevel::Breaking), count(ChangeLevel::Warning));
    let mut text = String::new();
    for e in entries {
        let c = &e.changes;
        match &e.page {
            Some(page) => _ = writeln!(text, "{app_id}.{}（页面 {page}）", c.tool),
            None => _ = writeln!(text, "{app_id}.{}", c.tool),
        }
        if c.removed && c.changes.is_empty() {
            _ = writeln!(text, "  已删除（此前已标 deprecated，兼容）");
        }
        for change in &c.changes {
            let path = if change.path.is_empty() { "-" } else { &change.path };
            _ = writeln!(text, "  {:<8}  {path}  {}", change.level.as_str(), change.message);
        }
    }
    let verdict = match (breaking, warning) {
        (0, 0) => "无破坏性或可能破坏的变化".to_owned(),
        (0, w) => format!("{w} 处可能破坏的变化（warning），无破坏性变化"),
        (b, w) => format!(
            "{b} 处破坏性变化（breaking）、{w} 处可能破坏：破坏性变更应改用新工具名，旧工具标 deprecated 并以 replacement 指向新工具（spec/protocol.md 3.7）"
        ),
    };
    _ = writeln!(text, "{verdict}");
    text
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn manifest(v: serde_json::Value) -> Manifest {
        let mut base = json!({"manifestVersion": 1, "appId": "shop", "name": "Shop"});
        for (k, val) in v.as_object().unwrap() {
            base[k] = val.clone();
        }
        serde_json::from_value(base).unwrap()
    }

    fn tool(name: &str, input: serde_json::Value) -> serde_json::Value {
        json!({"name": name, "description": "d", "inputSchema": input})
    }

    #[test]
    fn pairs_tools_by_name_across_pages() {
        let s = json!({"type": "object"});
        let req = json!({"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]});
        let old = manifest(json!({
            "tools": [tool("a", s.clone()), tool("moved", s.clone())],
            "pages": [{"name": "orders", "tools": [tool("o.list", s.clone())]}, {"name": "gone", "tools": [tool("g", s.clone())]}]
        }));
        let new = manifest(json!({
            "tools": [tool("a", s.clone())],
            "pages": [{"name": "orders", "tools": [tool("o.list", req), tool("moved", s.clone())]}]
        }));
        let got = compare_manifests(&old, &new);
        let keys: Vec<_> = got.iter().map(|e| (e.page.as_deref(), e.changes.tool.as_str(), e.changes.removed)).collect();
        assert_eq!(keys, [(Some("gone"), "g", true), (Some("orders"), "moved", false), (Some("orders"), "o.list", false)]);
        // 移动只记可能破坏，不算删除
        let moved = &got[1].changes;
        assert!(!moved.has_breaking());
        assert_eq!(moved.changes.len(), 1);
        assert!(moved.changes[0].message.contains("顶层") && moved.changes[0].message.contains("页面 orders"));
        assert!(got[0].changes.has_breaking() && got[2].changes.has_breaking());
        let text = render("shop", &got);
        assert!(text.contains("shop.o.list（页面 orders）"), "{text}");
        assert!(text.contains("2 处破坏性变化"), "{text}");
        let v = serde_json::to_value(&got[0]).unwrap();
        assert_eq!(v["page"], "gone");
        assert_eq!(v["tool"], "g");
        assert_eq!(v["changes"][0]["level"], "breaking");
    }

    #[test]
    fn render_without_changes() {
        assert_eq!(render("shop", &[]), "无破坏性或可能破坏的变化\n");
    }
}
