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

/// 参数级弃用只认 `deprecated: true`（JSON Schema 关键字）；嵌套对象与 `[T, null]` 外层上的声明同样生效。
#[test]
fn reads_property_deprecated_flag() {
    let model = model_for(json!({
        "type": "object",
        "properties": {
            "a": { "type": "string", "deprecated": true },
            "b": { "type": "string", "deprecated": false },
            "c": { "type": "string", "deprecated": "yes" },
            "d": { "type": "string" },
            "e": { "anyOf": [{ "type": "string" }, { "type": "null" }], "deprecated": true },
            "f": { "type": "object", "properties": { "g": { "type": "integer", "deprecated": true } } }
        }
    }));
    let flags: Vec<(&str, bool)> = ["a", "b", "c", "d", "e"].iter().map(|n| (*n, field(&model, n).deprecated)).collect();
    assert_eq!(flags, [("a", true), ("b", false), ("c", false), ("d", false), ("e", true)]);
    let Ty::Object(id) = field(&model, "f").ty else { panic!("f 应为对象") };
    assert!(model.object(id).expect("嵌套对象").fields[0].deprecated);
    assert!(model.has_deprecations());
    assert!(!model_for(json!({ "type": "object", "properties": { "d": { "type": "string" } } })).has_deprecations());
}
