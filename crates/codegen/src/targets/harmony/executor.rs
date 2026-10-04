//! 鸿蒙意图执行器文件与意图参数 schema。

use super::*;

mod convert;

pub(super) use convert::entity_unsupported;

pub(super) fn executor_file(model: &Model, tool: &ToolModel, domain: &str, ability: &str) -> String {
    let m = &model.module;
    let params = model.params(tool);
    let class = executor_name(model, tool);
    let lit = |s: &str| string_literal(Lang::TypeScript, s);

    let mut imports = BTreeSet::new();
    imports.insert(params.name.clone());
    imports.insert(format!("{m}ToolHandlers"));
    for f in &params.fields {
        convert::imports(model, f, &mut imports);
    }
    let has_entity = params
        .fields
        .iter()
        .any(|f| matches!(convert::repr(model, &f.ty), convert::Repr::Entity(_)));

    let mut c = Code::new("  ");
    let mut header = header_lines(model, "harmony-insight-intents");
    header.push(format!(
        "工具 `{}` 的意图执行器（insight_intent.json 的 insightIntentsSrcEntry 引用本文件）。",
        tool.info.name
    ));
    c.comment("// ", &header);
    c.blank();
    let entity_import = if has_entity { "InsightIntentEntity, " } else { "" };
    c.line(format!(
        "import {{ insightIntent, {entity_import}InsightIntentEntry, InsightIntentEntryExecutor }} from '@kit.AbilityKit';"
    ));
    c.line(format!(
        "import {{ {m}IntentRuntime }} from '../appmcp/{m}InsightIntents';"
    ));
    let names: Vec<String> = imports.into_iter().collect();
    c.line(format!(
        "import {{ {} }} from '../appmcp/{m}Tools';",
        names.join(", ")
    ));
    c.blank();
    convert::emit_entities(&mut c, model, tool, params);

    let mut doc = tool_doc(tool);
    if needs_confirmation(tool.info.risk) {
        doc.push(format!(
            "风险 {}：意图框架没有系统级确认，以前台模式执行，handler 应在执行前向用户确认。",
            risk_name(tool.info.risk)
        ));
    }
    c.block_doc(&doc);

    let display_description = display_description(tool);
    let mut keywords = vec![lit(tool.display_title())];
    if tool.info.title.is_some() {
        keywords.push(lit(&tool.info.name));
    }
    c.open("@InsightIntentEntry({");
    c.line(format!("intentName: {},", lit(&intent_name(model, tool))));
    c.line(format!("domain: {},", lit(domain)));
    c.line(format!("intentVersion: {},", lit(INTENT_VERSION)));
    c.line(format!("displayName: {},", lit(tool.display_title())));
    c.line(format!("displayDescription: {},", lit(display_description)));
    c.line(format!("llmDescription: {},", lit(&tool.info.description)));
    c.line(format!("keywords: [{}],", keywords.join(", ")));
    c.line(format!("abilityName: {},", lit(ability)));
    c.line(format!("executeMode: [{}],", execute_mode(tool)));
    let schema =
        serde_json::to_string_pretty(&intent_parameters(model, params)).unwrap_or_default();
    let mut lines = schema.lines();
    if let Some(first) = lines.next() {
        c.line(format!("parameters: {first}"));
        let rest: Vec<&str> = lines.collect();
        for (i, l) in rest.iter().enumerate() {
            let tail = if i + 1 == rest.len() { "," } else { "" };
            c.line(format!("{l}{tail}"));
        }
    }
    c.close("})");
    c.open(format!(
        "export default class {class} extends InsightIntentEntryExecutor<string> {{"
    ));
    for f in &params.fields {
        c.block_doc(&convert::prop_doc(model, f));
        c.line(format!("public {}?: {};", f.json_name, convert::prop_ty(model, f)));
    }
    if !params.fields.is_empty() {
        c.blank();
    }
    c.open("onExecute(): Promise<insightIntent.IntentResult<string>> {");
    let values: Vec<String> = params
        .fields
        .iter()
        .map(|f| format!("{}: {}", f.json_name, convert::emit_value(&mut c, model, f)))
        .collect();
    if values.is_empty() {
        c.line(format!("const params: {} = {{}};", params.name));
    } else {
        c.open(format!("const params: {} = {{", params.name));
        for v in &values {
            c.line(format!("{v},"));
        }
        c.close("};");
    }
    c.line(format!(
        "return {m}IntentRuntime.run((handlers: {m}ToolHandlers) => handlers.{}(params));",
        tool.camel
    ));
    c.close("}");
    c.close("}");
    c.finish()
}

/// 意图的 `displayDescription`：描述的第一行，为空时用标题。
pub(super) fn display_description(tool: &ToolModel) -> &str {
    let first_line = tool.info.description.lines().next().unwrap_or("").trim();
    if first_line.is_empty() {
        tool.display_title()
    } else {
        first_line
    }
}

/// 意图的 `parameters`：由类型模型重新生成的 JSON Schema。
///
/// @why 构建工具用 ajv（默认严格模式）编译 `parameters`：未知 `format` 与 `$ref` 会导致编译失败，
///   因此只输出类型、描述、枚举与数值 / 长度约束；降级为原始 JSON 的字段为空 schema（任意值）。
///
/// 顶层属性按执行器中的表示调整（整数为 `number`、字典与原始 JSON 为 JSON 文本，见 [`convert`]）；嵌套位置不变。
pub fn intent_parameters(model: &Model, params: &ObjectDecl) -> Value {
    let mut schema = object_schema(model, params);
    if let Some(Value::Object(props)) = schema.get_mut("properties") {
        for f in &params.fields {
            if let Some(Value::Object(s)) = props.get_mut(&f.json_name) {
                convert::patch_root_schema(model, f, s);
            }
        }
    }
    schema
}

fn object_schema(model: &Model, o: &ObjectDecl) -> Value {
    let mut schema = Map::new();
    schema.insert("type".into(), json!("object"));
    if let Some(d) = &o.description {
        schema.insert("description".into(), json!(d));
    }
    let mut props = Map::new();
    for f in &o.fields {
        props.insert(f.json_name.clone(), field_schema(model, f));
    }
    schema.insert("properties".into(), Value::Object(props));
    let required: Vec<&str> = o
        .fields
        .iter()
        .filter(|f| f.required)
        .map(|f| f.json_name.as_str())
        .collect();
    if !required.is_empty() {
        schema.insert("required".into(), json!(required));
    }
    Value::Object(schema)
}

fn ty_schema(model: &Model, t: &Ty) -> Map<String, Value> {
    let mut s = Map::new();
    match t {
        Ty::String => {
            s.insert("type".into(), json!("string"));
        }
        Ty::Integer => {
            s.insert("type".into(), json!("integer"));
        }
        Ty::Number => {
            s.insert("type".into(), json!("number"));
        }
        Ty::Boolean => {
            s.insert("type".into(), json!("boolean"));
        }
        Ty::Enum(id) => {
            s.insert("type".into(), json!("string"));
            if let Some(e) = model.enum_decl(*id) {
                s.insert("enum".into(), json!(e.values));
            }
        }
        Ty::Object(id) => {
            if let Some(o) = model.object(*id) {
                if let Value::Object(map) = object_schema(model, o) {
                    s = map;
                }
            }
        }
        Ty::Array(inner) => {
            s.insert("type".into(), json!("array"));
            s.insert("items".into(), Value::Object(ty_schema(model, inner)));
        }
        Ty::Map(inner) => {
            s.insert("type".into(), json!("object"));
            s.insert(
                "additionalProperties".into(),
                Value::Object(ty_schema(model, inner)),
            );
        }
        Ty::Json => {}
    }
    s
}

fn field_schema(model: &Model, f: &Field) -> Value {
    let mut s = ty_schema(model, &f.ty);
    if let Some(d) = &f.description {
        s.insert("description".into(), json!(d));
    }
    let c = &f.constraints;
    let numbers = [
        ("minimum", &c.minimum),
        ("maximum", &c.maximum),
        ("exclusiveMinimum", &c.exclusive_minimum),
        ("exclusiveMaximum", &c.exclusive_maximum),
    ];
    for (key, v) in numbers {
        if let Some(n) = v {
            s.insert(key.into(), Value::Number(n.clone()));
        }
    }
    let counts = [
        ("minLength", c.min_length),
        ("maxLength", c.max_length),
        ("minItems", c.min_items),
        ("maxItems", c.max_items),
    ];
    for (key, v) in counts {
        if let Some(n) = v {
            s.insert(key.into(), json!(n));
        }
    }
    if let Some(p) = &c.pattern {
        s.insert("pattern".into(), json!(p));
    }
    if let Some(values) = &c.allowed {
        s.insert("enum".into(), Value::Array(values.clone()));
    }
    if let Some(d) = &c.default {
        s.insert("default".into(), d.clone());
    }
    Value::Object(s)
}
