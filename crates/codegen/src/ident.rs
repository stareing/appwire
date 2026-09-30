//! 标识符转换与保留字处理。
//!
//! 工具名 `cart.checkout`、属性名 `user_id` 等统一先拆成小写单词，再按目标语言的命名风格拼接：
//! `CartCheckout`（Pascal）、`cartCheckout`（camel）、`cart_checkout`（snake）。
//! 非 ASCII 字母数字的字符视为分隔符；拼接结果为空时使用调用方给出的后备名。

use std::collections::HashSet;

/// 目标语言（用于保留字与转义规则）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    TypeScript,
    CSharp,
    Swift,
    Kotlin,
    Python,
    Dart,
}

/// 把任意名称拆成小写单词：按非字母数字字符与驼峰边界切分。
///
/// `cart.checkout` → `[cart, checkout]`；`removeItemV2` → `[remove, item, v2]`；
/// `HTTPServer` → `[http, server]`。
pub fn split_words(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    let chars: Vec<char> = name.chars().collect();
    let mut current = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_ascii_alphanumeric() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            continue;
        }
        if c.is_ascii_uppercase() && !current.is_empty() {
            let prev = chars[i - 1];
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_ascii_lowercase());
            // aB → 新词；ABc 中的 B → 新词（HTTPServer → HTTP | Server）
            if prev.is_ascii_lowercase()
                || prev.is_ascii_digit()
                || (prev.is_ascii_uppercase() && next_lower)
            {
                words.push(std::mem::take(&mut current));
            }
        }
        current.push(c.to_ascii_lowercase());
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

fn fix_leading_digit(s: String, prefix: &str) -> String {
    if s.starts_with(|c: char| c.is_ascii_digit()) {
        format!("{prefix}{s}")
    } else {
        s
    }
}

/// `cart.checkout` → `CartCheckout`。结果为空时返回 `fallback`。
pub fn pascal(name: &str, fallback: &str) -> String {
    let s: String = split_words(name).iter().map(|w| capitalize(w)).collect();
    if s.is_empty() {
        return fallback.to_string();
    }
    fix_leading_digit(s, "N")
}

/// `cart.checkout` → `cartCheckout`。
pub fn camel(name: &str, fallback: &str) -> String {
    let words = split_words(name);
    let mut s = String::new();
    for (i, w) in words.iter().enumerate() {
        if i == 0 {
            s.push_str(w);
        } else {
            s.push_str(&capitalize(w));
        }
    }
    if s.is_empty() {
        return fallback.to_string();
    }
    fix_leading_digit(s, "n")
}

/// `cart.checkout` → `cart_checkout`。
pub fn snake(name: &str, fallback: &str) -> String {
    let s = split_words(name).join("_");
    if s.is_empty() {
        return fallback.to_string();
    }
    fix_leading_digit(s, "n")
}

/// `cart.checkout` → `CART_CHECKOUT`。
pub fn screaming(name: &str, fallback: &str) -> String {
    snake(name, fallback).to_ascii_uppercase()
}

const TS_RESERVED: &[&str] = &[
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "new",
    "null",
    "return",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "as",
    "implements",
    "interface",
    "let",
    "package",
    "private",
    "protected",
    "public",
    "static",
    "yield",
    "any",
    "boolean",
    "number",
    "string",
    "symbol",
    "type",
    "unknown",
    "never",
    "object",
    "await",
    "async",
];

const CSHARP_RESERVED: &[&str] = &[
    "abstract",
    "as",
    "base",
    "bool",
    "break",
    "byte",
    "case",
    "catch",
    "char",
    "checked",
    "class",
    "const",
    "continue",
    "decimal",
    "default",
    "delegate",
    "do",
    "double",
    "else",
    "enum",
    "event",
    "explicit",
    "extern",
    "false",
    "finally",
    "fixed",
    "float",
    "for",
    "foreach",
    "goto",
    "if",
    "implicit",
    "in",
    "int",
    "interface",
    "internal",
    "is",
    "lock",
    "long",
    "namespace",
    "new",
    "null",
    "object",
    "operator",
    "out",
    "override",
    "params",
    "private",
    "protected",
    "public",
    "readonly",
    "ref",
    "return",
    "sbyte",
    "sealed",
    "short",
    "sizeof",
    "stackalloc",
    "static",
    "string",
    "struct",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "uint",
    "ulong",
    "unchecked",
    "unsafe",
    "ushort",
    "using",
    "virtual",
    "void",
    "volatile",
    "while",
];

const SWIFT_RESERVED: &[&str] = &[
    "associatedtype",
    "class",
    "deinit",
    "enum",
    "extension",
    "fileprivate",
    "func",
    "import",
    "init",
    "inout",
    "internal",
    "let",
    "open",
    "operator",
    "private",
    "precedencegroup",
    "protocol",
    "public",
    "rethrows",
    "static",
    "struct",
    "subscript",
    "typealias",
    "var",
    "break",
    "case",
    "catch",
    "continue",
    "default",
    "defer",
    "do",
    "else",
    "fallthrough",
    "for",
    "guard",
    "if",
    "in",
    "repeat",
    "return",
    "throw",
    "switch",
    "where",
    "while",
    "Any",
    "as",
    "await",
    "false",
    "is",
    "nil",
    "self",
    "Self",
    "super",
    "throws",
    "true",
    "try",
    "Type",
    "Protocol",
];

const KOTLIN_RESERVED: &[&str] = &[
    "as",
    "break",
    "class",
    "continue",
    "do",
    "else",
    "false",
    "for",
    "fun",
    "if",
    "in",
    "interface",
    "is",
    "null",
    "object",
    "package",
    "return",
    "super",
    "this",
    "throw",
    "true",
    "try",
    "typealias",
    "typeof",
    "val",
    "var",
    "when",
    "while",
];

const PYTHON_RESERVED: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue",
    "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import",
    "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while",
    "with", "yield", "match", "case", "type",
];

const DART_RESERVED: &[&str] = &[
    "abstract",
    "as",
    "assert",
    "async",
    "await",
    "base",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "covariant",
    "default",
    "deferred",
    "do",
    "dynamic",
    "else",
    "enum",
    "export",
    "extends",
    "extension",
    "external",
    "factory",
    "false",
    "final",
    "finally",
    "for",
    "Function",
    "get",
    "hide",
    "if",
    "implements",
    "import",
    "in",
    "interface",
    "is",
    "late",
    "library",
    "mixin",
    "new",
    "null",
    "of",
    "on",
    "operator",
    "part",
    "required",
    "rethrow",
    "return",
    "sealed",
    "set",
    "show",
    "static",
    "super",
    "switch",
    "sync",
    "this",
    "throw",
    "true",
    "try",
    "type",
    "typedef",
    "var",
    "void",
    "when",
    "while",
    "with",
    "yield",
];

/// 是否为该语言的保留字（含常用上下文关键字）。
pub fn is_reserved(lang: Lang, ident: &str) -> bool {
    let list = match lang {
        Lang::TypeScript => TS_RESERVED,
        Lang::CSharp => CSHARP_RESERVED,
        Lang::Swift => SWIFT_RESERVED,
        Lang::Kotlin => KOTLIN_RESERVED,
        Lang::Python => PYTHON_RESERVED,
        Lang::Dart => DART_RESERVED,
    };
    list.contains(&ident)
}

/// 对保留字转义：C# `@class`、Swift / Kotlin 反引号、TypeScript / Python / Dart 追加下划线。
pub fn escape(lang: Lang, ident: &str) -> String {
    if !is_reserved(lang, ident) {
        return ident.to_string();
    }
    match lang {
        Lang::CSharp => format!("@{ident}"),
        Lang::Swift | Lang::Kotlin => format!("`{ident}`"),
        Lang::TypeScript | Lang::Python | Lang::Dart => format!("{ident}_"),
    }
}

/// 名称分配器：保证同一作用域内的标识符唯一（冲突时追加数字后缀）。
#[derive(Debug, Default)]
pub struct NameScope {
    used: HashSet<String>,
}

impl NameScope {
    pub fn new() -> Self {
        Self::default()
    }

    /// 预先占用一批名称（如生成代码自身使用的成员名）。
    pub fn with_reserved(names: &[&str]) -> Self {
        Self {
            used: names.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// 分配一个唯一名称：`base` 已被占用时依次尝试 `base2`、`base3`……
    pub fn claim(&mut self, base: &str) -> String {
        if self.used.insert(base.to_string()) {
            return base.to_string();
        }
        let mut i = 2;
        loop {
            let candidate = format!("{base}{i}");
            if self.used.insert(candidate.clone()) {
                return candidate;
            }
            i += 1;
        }
    }
}

/// 是否是合法的 ASCII 标识符（字母或下划线开头，其后为字母数字下划线）。
pub fn is_plain_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '$' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_words() {
        assert_eq!(split_words("cart.checkout"), ["cart", "checkout"]);
        assert_eq!(split_words("todos.removeItem"), ["todos", "remove", "item"]);
        assert_eq!(split_words("HTTPServer"), ["http", "server"]);
        assert_eq!(split_words("get-order_v2"), ["get", "order", "v2"]);
        assert_eq!(split_words("收货地址"), Vec::<String>::new());
        assert_eq!(split_words("item2Name"), ["item2", "name"]);
    }

    #[test]
    fn converts_case() {
        assert_eq!(pascal("cart.checkout", "X"), "CartCheckout");
        assert_eq!(camel("cart.checkout", "x"), "cartCheckout");
        assert_eq!(snake("cart.checkout", "x"), "cart_checkout");
        assert_eq!(screaming("cart.checkout", "x"), "CART_CHECKOUT");
        assert_eq!(camel("Product_ID", "x"), "productId");
        assert_eq!(pascal("3d.view", "X"), "N3dView");
        assert_eq!(camel("2fa", "x"), "n2fa");
        assert_eq!(camel("备注", "field"), "field");
    }

    #[test]
    fn escapes_reserved_words() {
        assert_eq!(escape(Lang::CSharp, "class"), "@class");
        assert_eq!(escape(Lang::Swift, "default"), "`default`");
        assert_eq!(escape(Lang::Kotlin, "in"), "`in`");
        assert_eq!(escape(Lang::Python, "from"), "from_");
        assert_eq!(escape(Lang::Dart, "required"), "required_");
        assert_eq!(escape(Lang::TypeScript, "delete"), "delete_");
        assert_eq!(escape(Lang::Kotlin, "qty"), "qty");
    }

    #[test]
    fn name_scope_deduplicates() {
        let mut scope = NameScope::with_reserved(&["perform"]);
        assert_eq!(scope.claim("perform"), "perform2");
        assert_eq!(scope.claim("userId"), "userId");
        assert_eq!(scope.claim("userId"), "userId2");
        assert_eq!(scope.claim("userId"), "userId3");
    }

    #[test]
    fn plain_identifier() {
        assert!(is_plain_identifier("productId"));
        assert!(!is_plain_identifier("product-id"));
        assert!(!is_plain_identifier("1abc"));
        assert!(!is_plain_identifier("备注"));
    }
}
