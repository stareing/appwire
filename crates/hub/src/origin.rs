//! WebSocket `Origin` 允许列表。
//!
//! 模式写法：
//! - `http://localhost:*`：任意端口（也匹配不带端口的 `http://localhost`）；
//! - `https://app.example.com`：精确匹配；
//! - `*`：允许任意来源（仅用于调试）。

/// 默认允许的来源。
pub const DEFAULT_ALLOWED: &[&str] = &["http://localhost:*", "http://127.0.0.1:*"];

#[derive(Clone, Debug)]
pub struct OriginPolicy {
    patterns: Vec<String>,
}

impl Default for OriginPolicy {
    fn default() -> Self {
        Self::new(std::iter::empty::<String>())
    }
}

impl OriginPolicy {
    /// 默认模式 + 额外模式。
    pub fn new(extra: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let mut patterns: Vec<String> = DEFAULT_ALLOWED.iter().map(|s| s.to_string()).collect();
        patterns.extend(
            extra
                .into_iter()
                .map(|s| s.into().trim_end_matches('/').to_ascii_lowercase()),
        );
        Self { patterns }
    }

    /// 没有 `Origin` 头（非浏览器客户端）时允许。
    pub fn allows(&self, origin: Option<&str>) -> bool {
        match origin {
            None => true,
            Some(o) => {
                let o = o.trim_end_matches('/').to_ascii_lowercase();
                self.patterns.iter().any(|p| matches(p, &o))
            }
        }
    }
}

fn matches(pattern: &str, origin: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(base) = pattern.strip_suffix(":*") {
        if origin == base {
            return true;
        }
        return origin
            .strip_prefix(base)
            .and_then(|rest| rest.strip_prefix(':'))
            .is_some_and(|port| {
                !port.is_empty() && port.len() <= 5 && port.bytes().all(|b| b.is_ascii_digit())
            });
    }
    pattern == origin
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let p = OriginPolicy::default();
        assert!(p.allows(None));
        assert!(p.allows(Some("http://localhost:5173")));
        assert!(p.allows(Some("http://127.0.0.1:8080")));
        assert!(p.allows(Some("http://localhost")));
        assert!(p.allows(Some("HTTP://LOCALHOST:3000")));
        assert!(!p.allows(Some("https://localhost:5173")));
        assert!(!p.allows(Some("http://localhost.evil.com:80")));
        assert!(!p.allows(Some("http://localhost:80abc")));
        assert!(!p.allows(Some("http://evil.com")));
        assert!(!p.allows(Some("null")));
    }

    #[test]
    fn extra_patterns() {
        let p = OriginPolicy::new(["https://app.example.com", "https://dev.example.com:*"]);
        assert!(p.allows(Some("https://app.example.com")));
        assert!(p.allows(Some("https://app.example.com/")));
        assert!(!p.allows(Some("https://app.example.com:444")));
        assert!(p.allows(Some("https://dev.example.com:9000")));
        assert!(OriginPolicy::new(["*"]).allows(Some("https://anything")));
    }
}
