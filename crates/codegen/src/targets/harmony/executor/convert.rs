//! 自定义意图执行器的属性表示与参数转换。
//!
//! 构建工具按 `parameters` 的顶层属性校验执行器类属性（ets-loader `parseUserIntents.js`）：
//! - `processProperty`：非对象、非数组的属性类型取 `checker.typeToString`（类型别名保留别名名，`number` 永远不是 `integer`）；
//!   对象属性记为 `object`，只有带 `@InsightIntentEntity` 装饰器的类才是实体；非实体数组不记类型（不校验）；
//! - `schemaPropertiesValidation`：schema 的 `type` 与上述类型不等，或 schema 为 `object` 而属性不是实体时报 10110009；
//!   只校验顶层属性（嵌套对象、数组元素不校验）。
//!
//! 因此顶层属性按 [`Repr`] 改变表示，执行时校验并转换回工具参数类型；嵌套位置保持工具参数类型与原 schema。

use super::*;
use crate::schema::EnumDecl;
use crate::targets::harmony::entity::{ENTITY_ID, EntityProp, entity_class};

/// 顶层参数在执行器中的表示。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::targets::harmony) enum Repr<'m> {
    /// 与工具参数类型相同（字符串、数值、布尔、数组）。
    Same,
    /// 整数：属性为 `number`，`parameters` 中为 `number`，执行时校验为整数。
    Integer,
    /// 字符串枚举：属性为 `string`，执行时校验取值并转换为枚举类型。
    Enum(&'m EnumDecl),
    /// 对象：属性为生成的 `@InsightIntentEntity` 类，执行时转换为参数接口。
    Entity(&'m ObjectDecl),
    /// 字典（`Some(值类型)`）或原始 JSON（`None`）：属性为 JSON 文本，执行时解析。
    JsonText(Option<&'m Ty>),
}

pub(in crate::targets::harmony) fn repr<'m>(model: &'m Model, ty: &'m Ty) -> Repr<'m> {
    match ty {
        Ty::Integer => Repr::Integer,
        Ty::Enum(id) => model.enum_decl(*id).map_or(Repr::Same, Repr::Enum),
        Ty::Object(id) => model.object(*id).map_or(Repr::Same, Repr::Entity),
        Ty::Map(inner) => Repr::JsonText(Some(inner)),
        Ty::Json => Repr::JsonText(None),
        Ty::String | Ty::Number | Ty::Boolean | Ty::Array(_) => Repr::Same,
    }
}

/// 对象参数的实体类名：`<Module><对象类型名>Entity`。
pub(in crate::targets::harmony) fn entity_name(model: &Model, decl: &ObjectDecl) -> String {
    format!("{}{}Entity", model.module, decl.name)
}

/// 对象参数不能做成实体类的原因（嵌套属性名不是标识符，或与 `IntentEntity.entityId` 同名）。
pub(in crate::targets::harmony) fn entity_unsupported(model: &Model, f: &Field) -> Option<String> {
    let Repr::Entity(decl) = repr(model, &f.ty) else {
        return None;
    };
    decl.fields.iter().find_map(|inner| {
        if !ident::is_plain_identifier(&inner.json_name) {
            Some(format!(
                "参数 `{}` 的属性名 `{}` 不是标识符，无法作为意图实体属性",
                f.json_name, inner.json_name
            ))
        } else if inner.json_name == ENTITY_ID {
            Some(format!(
                "参数 `{}` 的属性 `{ENTITY_ID}` 与 IntentEntity 的成员同名",
                f.json_name
            ))
        } else {
            None
        }
    })
}

/// 执行器类属性的 ArkTS 类型。
pub(in crate::targets::harmony) fn prop_ty(model: &Model, f: &Field) -> String {
    let null = if f.nullable { " | null" } else { "" };
    match repr(model, &f.ty) {
        Repr::Same | Repr::Integer => field_ty(model, f),
        Repr::Enum(_) => format!("string{null}"),
        Repr::Entity(decl) => format!("{}{null}", entity_name(model, decl)),
        Repr::JsonText(_) => "string".to_string(),
    }
}

/// 执行器类属性的注释。
pub(in crate::targets::harmony) fn prop_doc(model: &Model, f: &Field) -> Vec<String> {
    let mut doc = field_doc(f);
    if let Repr::JsonText(_) = repr(model, &f.ty) {
        doc.push("意图参数为 JSON 文本，执行时解析。".to_string());
    }
    doc
}

/// 按表示调整 `parameters` 顶层属性的 schema。
pub(in crate::targets::harmony) fn patch_root_schema(
    model: &Model,
    f: &Field,
    schema: &mut Map<String, Value>,
) {
    let describe = |schema: &mut Map<String, Value>, note: &str| {
        let text = match &f.description {
            Some(d) => format!("{d}（{note}）"),
            None => note.to_string(),
        };
        schema.insert("description".into(), json!(text));
    };
    match repr(model, &f.ty) {
        Repr::Integer => {
            schema.insert("type".into(), json!("number"));
            describe(schema, "整数");
        }
        Repr::JsonText(map) => {
            schema.clear();
            schema.insert("type".into(), json!("string"));
            describe(
                schema,
                if map.is_some() {
                    "JSON 对象文本"
                } else {
                    "JSON 文本"
                },
            );
        }
        Repr::Same | Repr::Enum(_) | Repr::Entity(_) => {}
    }
}

/// 需从 Tools 文件导入的类型名（含实体属性引用的类型）。
pub(in crate::targets::harmony) fn imports(model: &Model, f: &Field, out: &mut BTreeSet<String>) {
    referenced_types(model, &f.ty, out);
    if let Repr::Entity(decl) = repr(model, &f.ty) {
        for inner in &decl.fields {
            referenced_types(model, &inner.ty, out);
        }
    }
}

/// 字段类型中直接引用的声明名。
fn referenced_types(model: &Model, t: &Ty, out: &mut BTreeSet<String>) {
    match t {
        Ty::Enum(id) | Ty::Object(id) => {
            out.insert(model.decl(*id).name().to_string());
        }
        Ty::Array(inner) | Ty::Map(inner) => referenced_types(model, inner, out),
        _ => {}
    }
}

fn lit(s: &str) -> String {
    string_literal(Lang::TypeScript, s)
}

fn converter_name(decl: &ObjectDecl) -> String {
    format!("to{}", decl.name)
}

fn required_names(decl: &ObjectDecl) -> Vec<&str> {
    decl.fields
        .iter()
        .filter(|f| f.required)
        .map(|f| f.json_name.as_str())
        .collect()
}

/// 对象参数的实体类与转换函数（执行器文件内、执行器类之前）。
pub(in crate::targets::harmony) fn emit_entities(
    c: &mut Code,
    model: &Model,
    tool: &ToolModel,
    params: &ObjectDecl,
) {
    for f in &params.fields {
        let Repr::Entity(decl) = repr(model, &f.ty) else {
            continue;
        };
        let class = entity_name(model, decl);
        let mut doc = decl_doc(decl.description.as_deref());
        doc.push(format!(
            "参数 `{}` 的意图实体（系统入口按 parameters 中该属性的 schema 赋值）。",
            f.json_name
        ));
        let props: Vec<EntityProp> = decl
            .fields
            .iter()
            .map(|inner| {
                EntityProp::new(&inner.json_name, field_ty(model, inner), field_doc(inner))
            })
            .collect();
        let category = format!("{}.{}", intent_name(model, tool), f.json_name);
        entity_class(c, &doc, &category, &class, &props);
        c.blank();
        emit_converter(c, decl, &class);
        c.blank();
    }
}

fn emit_converter(c: &mut Code, decl: &ObjectDecl, class: &str) {
    let required = required_names(decl);
    let ret = if required.is_empty() {
        decl.name.clone()
    } else {
        format!("{} | undefined", decl.name)
    };
    let note = if required.is_empty() {
        ""
    } else {
        "；缺少必填属性时为 undefined"
    };
    c.line(format!("/** {class} → {}{note}。 */", decl.name));
    c.open(format!(
        "function {}(entity: {class}): {ret} {{",
        converter_name(decl)
    ));
    let mut values = Vec::new();
    for inner in &decl.fields {
        if inner.required {
            let local = format!("{}Value", inner.json_name);
            c.line(format!("const {local} = entity.{};", inner.json_name));
            c.open(format!("if ({local} === undefined) {{"));
            c.line("return undefined;");
            c.close("}");
            values.push(format!("{}: {local}", inner.json_name));
        } else {
            values.push(format!("{0}: entity.{0}", inner.json_name));
        }
    }
    if values.is_empty() {
        c.line(format!("const converted: {} = {{}};", decl.name));
    } else {
        c.open(format!("const converted: {} = {{", decl.name));
        for v in &values {
            c.line(format!("{v},"));
        }
        c.close("};");
    }
    c.line("return converted;");
    c.close("}");
}

/// `local` 不存在（非必填时为 undefined、可空时为 null）时原样传递，否则取 `converted`。
fn when_present(local: &str, f: &Field, nullable: bool, converted: String) -> String {
    let mut absent = Vec::new();
    if !f.required {
        absent.push(format!("{local} === undefined"));
    }
    if nullable {
        absent.push(format!("{local} === null"));
    }
    if absent.is_empty() {
        converted
    } else {
        format!("{} ? {local} : {converted}", absent.join(" || "))
    }
}

/// 输出一个顶层参数的取值、必填检查与校验；返回参数对象中该属性的值表达式。
pub(in crate::targets::harmony) fn emit_value(c: &mut Code, model: &Model, f: &Field) -> String {
    let m = &model.module;
    let name = &f.json_name;
    let runtime = format!("{m}IntentRuntime");
    let r = repr(model, &f.ty);
    if r == Repr::Same && !f.required {
        return format!("this.{name}");
    }
    let suffix = match r {
        Repr::Entity(_) => "Entity",
        Repr::JsonText(_) => "Text",
        _ => "Value",
    };
    let local = format!("{name}{suffix}");
    c.line(format!("const {local} = this.{name};"));
    if f.required {
        c.open(format!("if ({local} === undefined) {{"));
        c.line(format!("return {runtime}.missing({});", lit(name)));
        c.close("}");
    }
    let invalid = |c: &mut Code, condition: String, reason: String| {
        c.open(format!("if ({condition}) {{"));
        c.line(format!(
            "return {runtime}.invalid({}, {});",
            lit(name),
            lit(&reason)
        ));
        c.close("}");
    };
    match r {
        Repr::Same => local,
        Repr::Integer => {
            invalid(
                c,
                format!("typeof {local} === 'number' && !Number.isInteger({local})"),
                "须为整数".to_string(),
            );
            local
        }
        Repr::Enum(e) => {
            let values: Vec<String> = e.values.iter().map(|v| lit(v)).collect();
            invalid(
                c,
                format!(
                    "typeof {local} === 'string' && ![{}].includes({local})",
                    values.join(", ")
                ),
                format!("取值须为 {}", e.values.join(" / ")),
            );
            when_present(&local, f, f.nullable, format!("{local} as {}", e.name))
        }
        Repr::Entity(decl) => {
            let value = format!("{name}Value");
            let converted = format!("{}({local})", converter_name(decl));
            c.line(format!(
                "const {value} = {};",
                when_present(&local, f, f.nullable, converted)
            ));
            let required = required_names(decl);
            if !required.is_empty() {
                // @why 必填时实体已排除 undefined，只判断转换结果才能让类型收窄为非 undefined
                let mut present = Vec::new();
                if !f.required {
                    present.push(format!("{local} !== undefined"));
                    if f.nullable {
                        present.push(format!("{local} !== null"));
                    }
                }
                present.push(format!("{value} === undefined"));
                invalid(
                    c,
                    present.join(" && "),
                    format!("缺少必填属性 {}", required.join(" / ")),
                );
            }
            value
        }
        Repr::JsonText(map) => {
            let (check, reason) = match map {
                Some(_) => (
                    format!("{runtime}.isJsonObject({local}, {})", f.nullable),
                    "须为 JSON 对象文本",
                ),
                None => (format!("{runtime}.isJson({local})"), "须为 JSON 文本"),
            };
            invalid(
                c,
                format!("typeof {local} === 'string' && !{check}"),
                reason.to_string(),
            );
            let parsed = format!("{runtime}.parseJson({local})");
            let converted = match map {
                Some(inner) => {
                    let null = if f.nullable { " | null" } else { "" };
                    format!("{parsed} as Record<string, {}>{null}", ty(model, inner))
                }
                None => parsed,
            };
            when_present(&local, f, false, converted)
        }
    }
}
