//! 「根对象下按名字存放 MCP 服务器」形式的 JSON 配置文件（Cursor `mcpServers`、VS Code `servers`、Gemini CLI `mcpServers`）。
//!
//! @invariant 只增删 `<root>.<name>`，其余内容原样保留（键顺序会按字母序重排：serde_json 未开启 preserve_order）。
//! @error 文件不是标准 JSON、或根对象 / 条目类型不对时报错，不改写文件。

use std::path::Path;

use serde_json::{Map, Value};

use super::Existing;
use crate::setup::files;

#[derive(Clone, Copy)]
pub struct JsonServers {
    /// 根对象下存放服务器表的键。
    pub root: &'static str,
    /// HTTP 服务器条目。
    pub entry: fn(url: &str) -> Value,
    /// 从条目取 HTTP URL；不是 HTTP 条目时为 `None`。
    pub url_of: fn(&Value) -> Option<String>,
}

impl JsonServers {
    fn load(&self, path: &Path) -> anyhow::Result<Map<String, Value>> {
        match files::read_json(path)? {
            None => Ok(Map::new()),
            Some(Value::Object(m)) => Ok(m),
            Some(_) => anyhow::bail!("{} 的顶层不是 JSON 对象，不自动修改", path.display()),
        }
    }

    fn servers<'a>(&self, doc: &'a Map<String, Value>, path: &Path) -> anyhow::Result<Option<&'a Map<String, Value>>> {
        match doc.get(self.root) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Object(m)) => Ok(Some(m)),
            Some(_) => anyhow::bail!("{} 中的 \"{}\" 不是对象，不自动修改", path.display(), self.root),
        }
    }

    pub fn read(&self, path: &Path, name: &str) -> anyhow::Result<Option<Existing>> {
        let doc = self.load(path)?;
        let Some(entry) = self.servers(&doc, path)?.and_then(|m| m.get(name)) else {
            return Ok(None);
        };
        Ok(Some(match (self.url_of)(entry) {
            Some(url) => Existing::http(&url),
            None => Existing { url: None, summary: entry.to_string(), replaceable: true },
        }))
    }

    pub fn insert(&self, path: &Path, name: &str, url: &str) -> anyhow::Result<()> {
        let mut doc = self.load(path)?;
        self.servers(&doc, path)?;
        let servers = doc
            .entry(self.root.to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
        if servers.is_null() {
            *servers = Value::Object(Map::new());
        }
        if let Value::Object(m) = servers {
            m.insert(name.to_owned(), (self.entry)(url));
        }
        files::write_json(path, &Value::Object(doc))
    }

    /// 删除条目；返回是否存在过。文件不存在或没有该条目时不写文件。
    pub fn remove(&self, path: &Path, name: &str) -> anyhow::Result<bool> {
        let mut doc = self.load(path)?;
        if self.servers(&doc, path)?.is_none_or(|m| !m.contains_key(name)) {
            return Ok(false);
        }
        if let Some(Value::Object(m)) = doc.get_mut(self.root) {
            m.remove(name);
        }
        files::write_json(path, &Value::Object(doc))?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const JS: JsonServers = JsonServers {
        root: "mcpServers",
        entry: |url| json!({ "url": url }),
        url_of: |v| v.get("url").and_then(Value::as_str).map(str::to_owned),
    };

    fn temp() -> std::path::PathBuf {
        let n: u64 = rand::random();
        let d = std::env::temp_dir().join(format!("appwire-js-{n:x}"));
        std::fs::create_dir_all(&d).unwrap();
        d.join("mcp.json")
    }

    #[test]
    fn insert_read_remove_keeps_others() {
        let f = temp();
        assert_eq!(JS.read(&f, "app-mcp").unwrap(), None);
        std::fs::write(&f, r#"{"theme":"dark","mcpServers":{"other":{"command":"x"}}}"#).unwrap();
        JS.insert(&f, "app-mcp", "http://127.0.0.1:7717/mcp").unwrap();
        assert_eq!(JS.read(&f, "app-mcp").unwrap(), Some(Existing::http("http://127.0.0.1:7717/mcp")));
        let other = JS.read(&f, "other").unwrap().unwrap();
        assert_eq!(other.url, None);
        assert!(JS.remove(&f, "app-mcp").unwrap());
        assert!(!JS.remove(&f, "app-mcp").unwrap());
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(v, json!({"theme":"dark","mcpServers":{"other":{"command":"x"}}}));
        let _ = std::fs::remove_dir_all(f.parent().unwrap());
    }

    #[test]
    fn creates_missing_file_and_root() {
        let f = temp();
        let _ = std::fs::remove_dir_all(f.parent().unwrap());
        JS.insert(&f, "app-mcp", "http://h/mcp").unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(v, json!({"mcpServers":{"app-mcp":{"url":"http://h/mcp"}}}));
        // 不存在的文件删除时不创建
        let g = f.with_file_name("none.json");
        assert!(!JS.remove(&g, "app-mcp").unwrap());
        assert!(!g.exists());
        let _ = std::fs::remove_dir_all(f.parent().unwrap());
    }

    #[test]
    fn refuses_non_standard_or_wrong_shape() {
        let f = temp();
        for bad in ["// c\n{}", "[1]", r#"{"mcpServers":[]}"#] {
            std::fs::write(&f, bad).unwrap();
            assert!(JS.insert(&f, "app-mcp", "http://h/mcp").is_err(), "{bad}");
            assert_eq!(std::fs::read_to_string(&f).unwrap(), bad, "失败时不改写");
        }
        let _ = std::fs::remove_dir_all(f.parent().unwrap());
    }
}
