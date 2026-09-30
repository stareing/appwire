//! 代码输出辅助：缩进、文档注释、字符串字面量转义。

use crate::ident::Lang;

/// 带缩进的文本缓冲区。
#[derive(Debug)]
pub struct Code {
    buf: String,
    level: usize,
    unit: &'static str,
}

impl Code {
    pub fn new(unit: &'static str) -> Self {
        Self {
            buf: String::new(),
            level: 0,
            unit,
        }
    }

    /// 输出一行（空字符串输出空行，不带缩进）。
    pub fn line(&mut self, text: impl AsRef<str>) {
        let text = text.as_ref();
        if !text.is_empty() {
            for _ in 0..self.level {
                self.buf.push_str(self.unit);
            }
            self.buf.push_str(text);
        }
        self.buf.push('\n');
    }

    pub fn blank(&mut self) {
        // 避免连续空行
        if !self.buf.is_empty() && !self.buf.ends_with("\n\n") {
            self.buf.push('\n');
        }
    }

    /// 输出一行并增加缩进（如 `class X {`）。
    pub fn open(&mut self, text: impl AsRef<str>) {
        self.line(text);
        self.level += 1;
    }

    /// 减少缩进并输出一行（如 `}`）。
    pub fn close(&mut self, text: impl AsRef<str>) {
        self.level = self.level.saturating_sub(1);
        self.line(text);
    }

    pub fn indent(&mut self) {
        self.level += 1;
    }

    pub fn dedent(&mut self) {
        self.level = self.level.saturating_sub(1);
    }

    /// 以 `prefix` 为前缀输出多行注释（如 `/// `、`// `、`# `）。
    pub fn comment(&mut self, prefix: &str, lines: &[String]) {
        for l in lines {
            if l.is_empty() {
                self.line(prefix.trim_end());
            } else {
                self.line(format!("{prefix}{l}"));
            }
        }
    }

    /// 输出 `/** ... */` 风格的文档注释（TypeScript、Kotlin）。
    pub fn block_doc(&mut self, lines: &[String]) {
        if lines.is_empty() {
            return;
        }
        let lines: Vec<String> = lines.iter().map(|l| l.replace("*/", "*\\/")).collect();
        if lines.len() == 1 {
            self.line(format!("/** {} */", lines[0]));
            return;
        }
        self.line("/**");
        for l in &lines {
            if l.is_empty() {
                self.line(" *");
            } else {
                self.line(format!(" * {l}"));
            }
        }
        self.line(" */");
    }

    /// 输出 C# XML 文档注释 `/// <summary>`。
    pub fn xml_doc(&mut self, lines: &[String]) {
        if lines.is_empty() {
            return;
        }
        self.line("/// <summary>");
        for l in lines {
            self.line(format!("/// {}", xml_escape(l)).trim_end());
        }
        self.line("/// </summary>");
    }

    pub fn finish(self) -> String {
        let mut s = self.buf;
        while s.ends_with("\n\n") {
            s.pop();
        }
        if !s.ends_with('\n') {
            s.push('\n');
        }
        s
    }
}

/// 把可能含换行的文本拆成注释行。
pub fn doc_lines(text: Option<&str>) -> Vec<String> {
    text.map(|t| t.lines().map(|l| l.trim_end().to_string()).collect())
        .unwrap_or_default()
}

pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// 生成目标语言的双引号字符串字面量。
pub fn string_literal(lang: Lang, s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '$' if matches!(lang, Lang::Kotlin | Lang::Dart) => out.push_str("\\$"),
            c if (c as u32) < 0x20 => match lang {
                Lang::Swift => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
                _ => out.push_str(&format!("\\u{:04x}", c as u32)),
            },
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// 生成文件头注释行（各 target 共用）。
pub fn header_lines(model: &crate::schema::Model, target: &str) -> Vec<String> {
    let mut lines = vec![
        format!("由 app-mcp-codegen 生成（target: {target}），请勿手动修改。"),
        format!("App：{}（{}）", model.app_name, model.app_id),
    ];
    if let Some(ov) = &model.overview {
        lines.push(format!("总览：{}", ov.summary.trim()));
    } else if let Some(d) = &model.app_description {
        lines.push(format!("简介：{}", d.trim()));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indents_and_trims() {
        let mut c = Code::new("  ");
        c.open("a {");
        c.line("b");
        c.blank();
        c.blank();
        c.close("}");
        c.blank();
        assert_eq!(c.finish(), "a {\n  b\n\n}\n");
    }

    #[test]
    fn escapes_literals() {
        assert_eq!(
            string_literal(Lang::Swift, "a\"b\\(x)\n"),
            r#""a\"b\\(x)\n""#
        );
        assert_eq!(string_literal(Lang::Kotlin, "$x"), r#""\$x""#);
        assert_eq!(string_literal(Lang::CSharp, "$x"), r#""$x""#);
        assert_eq!(string_literal(Lang::Swift, "\u{1}"), r#""\u{1}""#);
    }

    #[test]
    fn block_doc_escapes_terminator() {
        let mut c = Code::new("  ");
        c.block_doc(&["a */ b".to_string()]);
        assert_eq!(c.finish(), "/** a *\\/ b */\n");
    }
}
