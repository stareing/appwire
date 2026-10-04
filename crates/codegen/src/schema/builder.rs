//! 单个工具 schema 的递归转换（类型声明、枚举、约束、降级）。

use super::*;

pub(super) struct Builder {
    pub(super) types: Vec<TypeDecl>,
    pub(super) names: NameScope,
    pub(super) warnings: Vec<Warning>,
    pub(super) tool: String,
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
    pub(super) fn object_decl(
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
                    deprecated: prop.get("deprecated") == Some(&Value::Bool(true)),
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
