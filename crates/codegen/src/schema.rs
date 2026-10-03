//! JSON Schema → 语言无关的类型模型（各 target 共用）。
//!
//! 支持的构造：`type`（含 `["T", "null"]` 与 `nullable: true`）、`enum` / `const`、`required`、
//! `description` / `title`、数值与长度约束、`format`、`default`、嵌套 `properties`（生成嵌套类型）、
//! `items`（数组）、仅含 `additionalProperties` 的对象（字典）、`anyOf` / `oneOf` 中 `[T, null]` 形式。
//!
//! 不支持的构造（`$ref`、一般的 `oneOf` / `anyOf`、`allOf`、`not`、`if`、元组数组、多类型联合等）
//! 记录警告，并降级为"原始 JSON"类型。

use std::fmt;

use app_mcp_manifest::{Manifest, ToolInfo};
use app_mcp_protocol::AppOverview;
use serde_json::{Map, Number, Value};

use crate::ident::{self, NameScope};
use crate::ordered::Node;

mod builder;

use builder::Builder;

/// 代码生成过程中的警告（不影响输出，但提示开发者某处被降级）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Warning {
    /// 工具名；与具体工具无关时为空。
    pub tool: String,
    /// 在 inputSchema 中的位置，如 `/properties/shipping`。
    pub path: String,
    pub message: String,
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.tool.is_empty() {
            write!(f, "{}", self.message)
        } else if self.path.is_empty() {
            write!(f, "工具 {}：{}", self.tool, self.message)
        } else {
            write!(f, "工具 {} 的 {}：{}", self.tool, self.path, self.message)
        }
    }
}

/// 类型声明的索引（指向 [`Model::types`]）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TypeId(pub usize);

/// 字段类型。
#[derive(Clone, Debug, PartialEq)]
pub enum Ty {
    String,
    Integer,
    Number,
    Boolean,
    /// 字符串枚举。
    Enum(TypeId),
    /// 嵌套对象。
    Object(TypeId),
    Array(Box<Ty>),
    /// 键为字符串的字典（仅含 `additionalProperties` 的对象）。
    Map(Box<Ty>),
    /// 原始 JSON（任意值，或不支持的构造降级而来）。
    Json,
}

impl Ty {
    /// 是否为标量（字符串、数值、布尔、枚举）。
    pub fn is_scalar(&self) -> bool {
        matches!(
            self,
            Ty::String | Ty::Integer | Ty::Number | Ty::Boolean | Ty::Enum(_)
        )
    }
}

/// 字段上的附加约束，只用于生成注释或平台参数约束。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Constraints {
    pub minimum: Option<Number>,
    pub maximum: Option<Number>,
    pub exclusive_minimum: Option<Number>,
    pub exclusive_maximum: Option<Number>,
    pub min_length: Option<u64>,
    pub max_length: Option<u64>,
    pub min_items: Option<u64>,
    pub max_items: Option<u64>,
    pub pattern: Option<String>,
    pub format: Option<String>,
    pub default: Option<Value>,
    /// 非字符串枚举（如整数枚举）的取值，按基础类型生成并在注释中列出。
    pub allowed: Option<Vec<Value>>,
}

/// 对象的一个属性。
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    /// JSON 中的属性名（原样保留）。
    pub json_name: String,
    pub description: Option<String>,
    pub required: bool,
    /// 值可以为 `null`。
    pub nullable: bool,
    pub ty: Ty,
    pub constraints: Constraints,
    /// 降级原因（不支持的构造）。
    pub degraded: Option<String>,
}

impl Field {
    /// 在强类型语言中是否应生成为可空类型（非必填或可为 null）。
    pub fn optional(&self) -> bool {
        !self.required || self.nullable
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObjectDecl {
    /// PascalCase 类型名（各语言通用）。
    pub name: String,
    pub description: Option<String>,
    pub fields: Vec<Field>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnumDecl {
    pub name: String,
    pub description: Option<String>,
    /// 原始字符串取值（顺序与 schema 一致，已去重）。
    pub values: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeDecl {
    Object(ObjectDecl),
    Enum(EnumDecl),
}

impl TypeDecl {
    pub fn name(&self) -> &str {
        match self {
            TypeDecl::Object(o) => &o.name,
            TypeDecl::Enum(e) => &e.name,
        }
    }
}

/// 一个工具的模型。
#[derive(Clone, Debug, PartialEq)]
pub struct ToolModel {
    pub info: ToolInfo,
    /// `CartCheckout`
    pub pascal: String,
    /// `cartCheckout`
    pub camel: String,
    /// `cart_checkout`
    pub snake: String,
    /// 参数对象类型（`CartCheckoutParams`）。
    pub params: TypeId,
}

impl ToolModel {
    /// 标题：`title`，缺省时用工具名。
    pub fn display_title(&self) -> &str {
        self.info.title.as_deref().unwrap_or(&self.info.name)
    }
}

/// 整个清单的模型。
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub app_id: String,
    pub app_name: String,
    pub app_description: Option<String>,
    pub overview: Option<AppOverview>,
    /// 模块名（PascalCase），用于 `<Module>ToolHandlers` 等。
    pub module: String,
    pub tools: Vec<ToolModel>,
    /// 类型声明，按依赖顺序排列（被引用的类型在前）。
    pub types: Vec<TypeDecl>,
    pub warnings: Vec<Warning>,
}

impl Model {
    pub fn decl(&self, id: TypeId) -> &TypeDecl {
        &self.types[id.0]
    }

    pub fn object(&self, id: TypeId) -> Option<&ObjectDecl> {
        match &self.types[id.0] {
            TypeDecl::Object(o) => Some(o),
            TypeDecl::Enum(_) => None,
        }
    }

    pub fn enum_decl(&self, id: TypeId) -> Option<&EnumDecl> {
        match &self.types[id.0] {
            TypeDecl::Enum(e) => Some(e),
            TypeDecl::Object(_) => None,
        }
    }

    /// 某个工具的参数对象。
    pub fn params(&self, tool: &ToolModel) -> &ObjectDecl {
        // 参数类型总是以对象声明创建（见 build）；取不到时返回空对象而不是 panic
        self.object(tool.params).unwrap_or(&EMPTY_OBJECT)
    }

    /// 是否有任何字段用到原始 JSON 类型。
    pub fn uses_json(&self) -> bool {
        fn ty_uses(ty: &Ty) -> bool {
            match ty {
                Ty::Json => true,
                Ty::Array(inner) | Ty::Map(inner) => ty_uses(inner),
                _ => false,
            }
        }
        self.types.iter().any(|t| match t {
            TypeDecl::Object(o) => o.fields.iter().any(|f| ty_uses(&f.ty)),
            TypeDecl::Enum(_) => false,
        })
    }
}

static EMPTY_OBJECT: ObjectDecl = ObjectDecl {
    name: String::new(),
    description: None,
    fields: Vec::new(),
};

/// 生成代码中固定使用的类型名后缀（避免与生成的类型冲突）。
pub fn reserved_type_names(module: &str) -> Vec<String> {
    vec![
        format!("{module}ToolHandlers"),
        format!("{module}Tools"),
        format!("{module}AppFunctions"),
        format!("{module}AppShortcuts"),
        format!("{module}IntentRuntime"),
        format!("{module}ActionProvider"),
        format!("I{module}ToolHandlers"),
        format!("{module}ToolError"),
        format!("{module}ToolParams"),
        format!("{module}ToolName"),
        format!("{module}ToolHandlersProvider"),
        format!("{module}AppFunctionService"),
        format!("Base{module}AppFunctionService"),
        "JSONValue".to_string(),
    ]
}

/// 从清单构建模型。`module` 为空时由 `appId` 推导（`shop` → `Shop`）。
///
/// `order` 为清单原文的键顺序树（见 [`crate::ordered`]）；给出时字段按清单中声明的顺序排列，
/// 否则按键名排序。
pub fn build(manifest: &Manifest, module: Option<&str>, order: Option<&Node>) -> Model {
    build_filtered(manifest, module, order, |_| true)
}

/// 同 [`build`]，只为 `keep` 为真的工具建模（被跳过工具的参数类型不生成）。
pub fn build_filtered(
    manifest: &Manifest,
    module: Option<&str>,
    order: Option<&Node>,
    keep: impl Fn(&ToolInfo) -> bool,
) -> Model {
    let module = match module {
        Some(m) if !m.is_empty() => ident::pascal(m, "App"),
        _ => ident::pascal(&manifest.app_id, "App"),
    };
    let reserved = reserved_type_names(&module);
    let reserved_refs: Vec<&str> = reserved.iter().map(String::as_str).collect();
    let mut builder = Builder {
        types: Vec::new(),
        names: NameScope::with_reserved(&reserved_refs),
        warnings: Vec::new(),
        tool: String::new(),
    };

    let mut method_names = NameScope::new();
    let mut tools = Vec::new();
    for (index, info) in manifest.tools.iter().enumerate().filter(|(_, t)| keep(t)) {
        let tool_order = order
            .and_then(|n| n.get("tools"))
            .and_then(|t| t.index(index))
            .and_then(|t| t.get("inputSchema"));
        builder.tool = info.name.clone();
        let pascal = builder.names.claim(&ident::pascal(&info.name, "Tool"));
        // 参数类型名与工具的 Pascal 名同步（`CartCheckout` → `CartCheckoutParams`）
        let camel = method_names.claim(&ident::camel(&info.name, "tool"));
        let snake = ident::snake(&camel, "tool");
        let params = builder.object_decl(
            &info.input_schema,
            tool_order,
            &pascal,
            "Params",
            Some(info.description.clone()),
            "",
        );
        tools.push(ToolModel {
            info: info.clone(),
            pascal,
            camel,
            snake,
            params,
        });
    }

    Model {
        app_id: manifest.app_id.clone(),
        app_name: manifest.name.clone(),
        app_description: manifest.description.clone(),
        overview: manifest.overview.clone(),
        module,
        tools,
        types: builder.types,
        warnings: builder.warnings,
    }
}

/// 字段的附加说明（约束、格式、默认值、降级原因），用于各语言的文档注释。
pub fn field_notes(field: &Field) -> Vec<String> {
    let c = &field.constraints;
    let mut notes = Vec::new();
    let mut range = Vec::new();
    if let Some(v) = &c.minimum {
        range.push(format!("≥ {v}"));
    }
    if let Some(v) = &c.exclusive_minimum {
        range.push(format!("> {v}"));
    }
    if let Some(v) = &c.maximum {
        range.push(format!("≤ {v}"));
    }
    if let Some(v) = &c.exclusive_maximum {
        range.push(format!("< {v}"));
    }
    if !range.is_empty() {
        notes.push(format!("取值范围：{}", range.join("，")));
    }
    if let Some(values) = &c.allowed {
        let list: Vec<String> = values.iter().map(Value::to_string).collect();
        notes.push(format!("可选值：{}", list.join(", ")));
    }
    if c.min_length.is_some() || c.max_length.is_some() {
        notes.push(format!("长度：{}", bounds(c.min_length, c.max_length)));
    }
    if c.min_items.is_some() || c.max_items.is_some() {
        notes.push(format!("元素个数：{}", bounds(c.min_items, c.max_items)));
    }
    if let Some(f) = &c.format {
        notes.push(format!("格式：{f}"));
    }
    if let Some(p) = &c.pattern {
        notes.push(format!("正则：{p}"));
    }
    if let Some(d) = &c.default {
        notes.push(format!("默认值：{d}"));
    }
    if let Some(reason) = &field.degraded {
        notes.push(format!("原始 JSON（{reason}）"));
    }
    notes
}

fn bounds(min: Option<u64>, max: Option<u64>) -> String {
    match (min, max) {
        (Some(a), Some(b)) => format!("{a}–{b}"),
        (Some(a), None) => format!("≥ {a}"),
        (None, Some(b)) => format!("≤ {b}"),
        (None, None) => String::new(),
    }
}

/// 字段的完整文档注释行：描述 + 附加说明。
pub fn field_doc(field: &Field) -> Vec<String> {
    let mut lines: Vec<String> = field
        .description
        .as_deref()
        .map(|d| d.lines().map(str::to_string).collect())
        .unwrap_or_default();
    let notes = field_notes(field);
    if !notes.is_empty() {
        lines.push(notes.join("；"));
    }
    lines
}

#[cfg(test)]
mod tests;
