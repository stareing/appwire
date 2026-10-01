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

struct Builder {
    types: Vec<TypeDecl>,
    names: NameScope,
    warnings: Vec<Warning>,
    tool: String,
}

/// 单个 schema 节点的转换结果。
struct Converted {
    ty: Ty,
    nullable: bool,
    constraints: Constraints,
    degraded: Option<String>,
}

impl Converted {
    fn plain(ty: Ty) -> Self {
        Self {
            ty,
            nullable: false,
            constraints: Constraints::default(),
            degraded: None,
        }
    }
}

const UNSUPPORTED_KEYWORDS: &[&str] = &[
    "$ref",
    "$dynamicRef",
    "allOf",
    "not",
    "if",
    "then",
    "else",
    "dependentSchemas",
];

impl Builder {
    fn warn(&mut self, path: &str, message: impl Into<String>) {
        self.warnings.push(Warning {
            tool: self.tool.clone(),
            path: if path.is_empty() {
                "/".to_string()
            } else {
                path.to_string()
            },
            message: message.into(),
        });
    }

    fn degrade(&mut self, path: &str, reason: String) -> Converted {
        self.warn(path, format!("{reason}，降级为原始 JSON"));
        Converted {
            degraded: Some(reason),
            ..Converted::plain(Ty::Json)
        }
    }

    /// 创建对象声明。`prefix` + `suffix` 为类型名（如 `CartCheckout` + `Params`）；
    /// 嵌套类型名为 `prefix` + 属性名（`CartCheckout` + `Shipping`）。
    fn object_decl(
        &mut self,
        schema: &Value,
        order: Option<&Node>,
        prefix: &str,
        suffix: &str,
        description: Option<String>,
        path: &str,
    ) -> TypeId {
        let name = self.names.claim(&format!("{prefix}{suffix}"));
        // 先占位，保证嵌套类型排在前面时索引仍然稳定
        let empty = Map::new();
        let obj = schema.as_object().unwrap_or(&empty);
        let required: Vec<&str> = obj
            .get("required")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let mut fields = Vec::new();
        if let Some(props) = obj.get("properties").and_then(Value::as_object) {
            let props_order = order.and_then(|n| n.get("properties"));
            // 按原文顺序排列；键集合不一致（不应发生）时退回按键名排序
            let mut keys: Vec<&str> = props_order.map(Node::keys).unwrap_or_default();
            if keys.len() != props.len() || !keys.iter().all(|k| props.contains_key(*k)) {
                keys = props.keys().map(String::as_str).collect();
            }
            for key in keys {
                let Some(prop) = props.get(key) else { continue };
                let prop_order = props_order.and_then(|n| n.get(key));
                let key = &key.to_string();
                let prop_path = format!("{path}/properties/{key}");
                let child_prefix = format!("{prefix}{}", ident::pascal(key, "Field"));
                let converted = self.convert(prop, prop_order, &child_prefix, &prop_path);
                let description = prop
                    .get("description")
                    .or_else(|| prop.get("title"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                fields.push(Field {
                    json_name: key.clone(),
                    description,
                    required: required.contains(&key.as_str()),
                    nullable: converted.nullable,
                    ty: converted.ty,
                    constraints: converted.constraints,
                    degraded: converted.degraded,
                });
            }
        } else if obj
            .get("additionalProperties")
            .is_some_and(Value::is_object)
            && path.is_empty()
        {
            self.warn(
                path,
                "参数根对象只声明了 additionalProperties，生成的参数类型不含这些键",
            );
        }
        for key in &required {
            if !fields.iter().any(|f| f.json_name == *key) {
                self.warn(
                    path,
                    format!("required 中的 `{key}` 未在 properties 中声明，已忽略"),
                );
            }
        }
        self.types.push(TypeDecl::Object(ObjectDecl {
            name,
            description,
            fields,
        }));
        TypeId(self.types.len() - 1)
    }

    fn enum_decl(
        &mut self,
        prefix: &str,
        description: Option<String>,
        values: Vec<String>,
    ) -> TypeId {
        let name = self.names.claim(prefix);
        let mut unique: Vec<String> = Vec::new();
        for v in values {
            if !unique.contains(&v) {
                unique.push(v);
            }
        }
        self.types.push(TypeDecl::Enum(EnumDecl {
            name,
            description,
            values: unique,
        }));
        TypeId(self.types.len() - 1)
    }

    fn convert(
        &mut self,
        schema: &Value,
        order: Option<&Node>,
        prefix: &str,
        path: &str,
    ) -> Converted {
        let obj = match schema {
            Value::Object(o) => o,
            // `true` / `{}`：任意值
            Value::Bool(true) => return Converted::plain(Ty::Json),
            _ => return self.degrade(path, "schema 不是对象".to_string()),
        };

        for kw in UNSUPPORTED_KEYWORDS {
            if obj.contains_key(*kw) {
                return self.degrade(path, format!("不支持 `{kw}`"));
            }
        }

        // anyOf / oneOf：只支持 [T, {type: null}]
        for kw in ["anyOf", "oneOf"] {
            if let Some(variants) = obj.get(kw) {
                let Some(list) = variants.as_array() else {
                    return self.degrade(path, format!("`{kw}` 不是数组"));
                };
                let non_null: Vec<(usize, &Value)> = list
                    .iter()
                    .enumerate()
                    .filter(|(_, v)| !is_null_schema(v))
                    .collect();
                if non_null.len() == 1 && list.len() == 2 {
                    let (variant_index, variant) = non_null[0];
                    let variant_order = order
                        .and_then(|n| n.get(kw))
                        .and_then(|n| n.index(variant_index));
                    let mut merged = variant.clone();
                    // 外层的 description 等保留在字段上，这里只合并内层 schema
                    if let (Some(m), Some(desc)) = (merged.as_object_mut(), obj.get("description"))
                    {
                        m.entry("description").or_insert(desc.clone());
                    }
                    let mut inner =
                        self.convert(&merged, variant_order, prefix, &format!("{path}/{kw}"));
                    inner.nullable = true;
                    return inner;
                }
                return self.degrade(path, format!("不支持一般形式的 `{kw}`"));
            }
        }

        let mut nullable = obj
            .get("nullable")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut constraints = read_constraints(obj);

        // const → 单值枚举
        if let Some(c) = obj.get("const") {
            return self.convert_enum(
                std::slice::from_ref(c),
                obj,
                prefix,
                path,
                nullable,
                constraints,
            );
        }
        if let Some(values) = obj.get("enum") {
            let Some(list) = values.as_array() else {
                return self.degrade(path, "`enum` 不是数组".to_string());
            };
            return self.convert_enum(list, obj, prefix, path, nullable, constraints);
        }

        // type
        let type_name: Option<String> = match obj.get("type") {
            None => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Array(list)) => {
                let names: Vec<&str> = list.iter().filter_map(Value::as_str).collect();
                if names.contains(&"null") {
                    nullable = true;
                }
                let non_null: Vec<&str> = names.into_iter().filter(|n| *n != "null").collect();
                match non_null.as_slice() {
                    [] => None,
                    [one] => Some(one.to_string()),
                    many => {
                        return self
                            .degrade(path, format!("不支持多类型联合 {}", many.join(" | ")));
                    }
                }
            }
            Some(_) => return self.degrade(path, "`type` 既不是字符串也不是数组".to_string()),
        };
        let type_name = type_name.or_else(|| {
            if obj.contains_key("properties") {
                Some("object".to_string())
            } else if obj.contains_key("items") {
                Some("array".to_string())
            } else {
                None
            }
        });

        if obj.contains_key("prefixItems") {
            return self.degrade(path, "不支持 `prefixItems`".to_string());
        }
        let description = obj
            .get("description")
            .or_else(|| obj.get("title"))
            .and_then(Value::as_str)
            .map(str::to_string);

        let ty = match type_name.as_deref() {
            None => Ty::Json,
            Some("string") => Ty::String,
            Some("integer") => Ty::Integer,
            Some("number") => Ty::Number,
            Some("boolean") => Ty::Boolean,
            Some("null") => Ty::Json,
            Some("object") => {
                if obj.get("properties").is_some_and(Value::is_object) {
                    Ty::Object(self.object_decl(schema, order, prefix, "", description, path))
                } else {
                    match obj.get("additionalProperties") {
                        Some(ap @ Value::Object(_)) => {
                            let inner = self.convert(
                                ap,
                                order.and_then(|n| n.get("additionalProperties")),
                                &format!("{prefix}Value"),
                                &format!("{path}/additionalProperties"),
                            );
                            if inner.nullable {
                                self.warn(path, "字典的值可为 null，生成的类型忽略 null");
                            }
                            Ty::Map(Box::new(inner.ty))
                        }
                        _ => Ty::Json,
                    }
                }
            }
            Some("array") => match obj.get("items") {
                None | Some(Value::Bool(true)) => Ty::Array(Box::new(Ty::Json)),
                Some(items @ Value::Object(_)) => {
                    let inner = self.convert(
                        items,
                        order.and_then(|n| n.get("items")),
                        &format!("{prefix}Item"),
                        &format!("{path}/items"),
                    );
                    if inner.nullable {
                        self.warn(path, "数组元素可为 null，生成的类型忽略 null");
                    }
                    // 元素约束写进字段注释意义不大，丢弃
                    Ty::Array(Box::new(inner.ty))
                }
                Some(_) => return self.degrade(path, "不支持元组形式的 `items`".to_string()),
            },
            Some(other) => return self.degrade(path, format!("未知类型 `{other}`")),
        };
        if !matches!(ty, Ty::String) {
            constraints.format = None;
        }
        Converted {
            ty,
            nullable,
            constraints,
            degraded: None,
        }
    }

    fn convert_enum(
        &mut self,
        values: &[Value],
        obj: &Map<String, Value>,
        prefix: &str,
        path: &str,
        mut nullable: bool,
        mut constraints: Constraints,
    ) -> Converted {
        let non_null: Vec<&Value> = values.iter().filter(|v| !v.is_null()).collect();
        if non_null.len() != values.len() {
            nullable = true;
        }
        if non_null.is_empty() {
            return self.degrade(path, "枚举没有非 null 取值".to_string());
        }
        let description = obj
            .get("description")
            .or_else(|| obj.get("title"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let ty = if non_null.iter().all(|v| v.is_string()) {
            let strings: Vec<String> = non_null
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            Ty::Enum(self.enum_decl(prefix, description, strings))
        } else if non_null.iter().all(|v| v.is_i64() || v.is_u64()) {
            constraints.allowed = Some(non_null.into_iter().cloned().collect());
            Ty::Integer
        } else if non_null.iter().all(|v| v.is_number()) {
            constraints.allowed = Some(non_null.into_iter().cloned().collect());
            Ty::Number
        } else if non_null.iter().all(|v| v.is_boolean()) {
            constraints.allowed = Some(non_null.into_iter().cloned().collect());
            Ty::Boolean
        } else {
            return self.degrade(path, "枚举取值类型不一致".to_string());
        };
        Converted {
            ty,
            nullable,
            constraints,
            degraded: None,
        }
    }
}

fn is_null_schema(v: &Value) -> bool {
    v.get("type").and_then(Value::as_str) == Some("null")
}

fn read_constraints(obj: &Map<String, Value>) -> Constraints {
    let num = |k: &str| obj.get(k).and_then(Value::as_number).cloned();
    let uint = |k: &str| obj.get(k).and_then(Value::as_u64);
    Constraints {
        minimum: num("minimum"),
        maximum: num("maximum"),
        exclusive_minimum: num("exclusiveMinimum"),
        exclusive_maximum: num("exclusiveMaximum"),
        min_length: uint("minLength"),
        max_length: uint("maxLength"),
        min_items: uint("minItems"),
        max_items: uint("maxItems"),
        pattern: obj
            .get("pattern")
            .and_then(Value::as_str)
            .map(str::to_string),
        format: obj
            .get("format")
            .and_then(Value::as_str)
            .map(str::to_string),
        default: obj.get("default").cloned(),
        allowed: None,
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
mod tests {
    use super::*;
    use serde_json::json;

    fn model_for(schema: Value) -> Model {
        let manifest = app_mcp_manifest::parse(
            &json!({
                "manifestVersion": 1,
                "appId": "shop",
                "name": "Shop",
                "tools": [{ "name": "cart.checkout", "description": "结算", "inputSchema": schema }]
            })
            .to_string(),
        )
        .expect("清单");
        build(&manifest, None, None)
    }

    fn field<'a>(model: &'a Model, name: &str) -> &'a Field {
        let params = model.params(&model.tools[0]);
        params
            .fields
            .iter()
            .find(|f| f.json_name == name)
            .expect("字段")
    }

    #[test]
    fn maps_scalars_required_and_nullable() {
        let model = model_for(json!({
            "type": "object",
            "properties": {
                "a": { "type": "string", "description": "甲" },
                "b": { "type": "integer", "minimum": 1, "maximum": 9 },
                "c": { "type": ["number", "null"] },
                "d": { "type": "boolean", "nullable": true },
                "e": { "anyOf": [{ "type": "string" }, { "type": "null" }] }
            },
            "required": ["a", "c"]
        }));
        assert_eq!(model.tools[0].pascal, "CartCheckout");
        assert_eq!(model.params(&model.tools[0]).name, "CartCheckoutParams");
        let a = field(&model, "a");
        assert_eq!(
            (a.ty.clone(), a.required, a.nullable),
            (Ty::String, true, false)
        );
        assert_eq!(a.description.as_deref(), Some("甲"));
        let b = field(&model, "b");
        assert_eq!(b.ty, Ty::Integer);
        assert!(b.optional());
        assert_eq!(field_notes(b), ["取值范围：≥ 1，≤ 9"]);
        let c = field(&model, "c");
        assert_eq!(
            (c.ty.clone(), c.required, c.nullable),
            (Ty::Number, true, true)
        );
        assert!(field(&model, "d").nullable);
        let e = field(&model, "e");
        assert_eq!((e.ty.clone(), e.nullable), (Ty::String, true));
        assert!(model.warnings.is_empty(), "{:?}", model.warnings);
    }

    #[test]
    fn maps_enums_objects_arrays_maps() {
        let model = model_for(json!({
            "type": "object",
            "properties": {
                "method": { "enum": ["standard", "express", "standard"] },
                "level": { "type": "integer", "enum": [1, 2, 3] },
                "shipping": {
                    "type": "object",
                    "properties": { "city": { "type": "string" } },
                    "required": ["city"]
                },
                "items": { "type": "array", "items": { "type": "object", "properties": { "id": { "type": "string" } } } },
                "tags": { "type": "array", "items": { "type": "string" } },
                "meta": { "type": "object", "additionalProperties": { "type": "integer" } },
                "any": {}
            }
        }));
        let method = field(&model, "method");
        let Ty::Enum(id) = method.ty else {
            panic!("应为枚举")
        };
        let decl = model.enum_decl(id).expect("枚举");
        assert_eq!(decl.name, "CartCheckoutMethod");
        assert_eq!(decl.values, ["standard", "express"]);
        let level = field(&model, "level");
        assert_eq!(level.ty, Ty::Integer);
        assert_eq!(field_notes(level), ["可选值：1, 2, 3"]);
        let Ty::Object(ship) = field(&model, "shipping").ty else {
            panic!("应为对象")
        };
        let ship = model.object(ship).expect("对象");
        assert_eq!(ship.name, "CartCheckoutShipping");
        assert!(ship.fields[0].required);
        let Ty::Array(inner) = &field(&model, "items").ty else {
            panic!("应为数组")
        };
        let Ty::Object(item) = **inner else {
            panic!("元素应为对象")
        };
        assert_eq!(
            model.object(item).expect("对象").name,
            "CartCheckoutItemsItem"
        );
        assert_eq!(field(&model, "tags").ty, Ty::Array(Box::new(Ty::String)));
        assert_eq!(field(&model, "meta").ty, Ty::Map(Box::new(Ty::Integer)));
        assert_eq!(field(&model, "any").ty, Ty::Json);
        // 被引用的类型排在参数类型之前
        let params_index = model.tools[0].params.0;
        assert!(item.0 < params_index);
        assert!(model.warnings.is_empty());
        assert!(model.uses_json());
    }

    #[test]
    fn degrades_unsupported_constructs_with_warnings() {
        let model = model_for(json!({
            "type": "object",
            "properties": {
                "r": { "$ref": "#/$defs/x" },
                "o": { "oneOf": [{ "type": "string" }, { "type": "integer" }] },
                "u": { "type": ["string", "integer"] },
                "t": { "type": "array", "items": [{ "type": "string" }] },
                "ok": { "oneOf": [{ "type": "string" }, { "type": "null" }] }
            },
            "required": ["missing"]
        }));
        for name in ["r", "o", "u", "t"] {
            let f = field(&model, name);
            assert_eq!(f.ty, Ty::Json, "{name}");
            assert!(f.degraded.is_some(), "{name}");
        }
        assert_eq!(field(&model, "ok").ty, Ty::String);
        let messages: Vec<String> = model.warnings.iter().map(ToString::to_string).collect();
        assert_eq!(messages.len(), 5, "{messages:?}");
        // 未提供键顺序时按键名排序：o, r, t, u
        assert!(messages[1].contains("/properties/r"), "{messages:?}");
        assert!(messages[1].contains("$ref"));
        assert!(messages.iter().any(|m| m.contains("missing")));
        assert!(field_notes(field(&model, "r"))[0].starts_with("原始 JSON"));
    }

    #[test]
    fn type_names_are_unique() {
        let manifest = app_mcp_manifest::parse(
            &json!({
                "manifestVersion": 1,
                "appId": "shop",
                "name": "Shop",
                "tools": [
                    { "name": "cart.add", "description": "a", "inputSchema": { "type": "object" } },
                    { "name": "cart_add", "description": "b", "inputSchema": { "type": "object" } },
                    { "name": "shop.tool-handlers", "description": "c", "inputSchema": { "type": "object" } }
                ]
            })
            .to_string(),
        )
        .expect("清单");
        let model = build(&manifest, None, None);
        let names: Vec<&str> = model.tools.iter().map(|t| t.pascal.as_str()).collect();
        assert_eq!(names, ["CartAdd", "CartAdd2", "ShopToolHandlers2"]);
        let methods: Vec<&str> = model.tools.iter().map(|t| t.camel.as_str()).collect();
        assert_eq!(methods, ["cartAdd", "cartAdd2", "shopToolHandlers"]);
        assert_eq!(model.module, "Shop");
        assert_eq!(build(&manifest, Some("my-shop"), None).module, "MyShop");
    }
}
