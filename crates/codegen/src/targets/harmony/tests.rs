use super::*;
use app_mcp_protocol::Risk;
use serde_json::json;

fn model(tools: Value) -> Model {
    let manifest = app_mcp_manifest::parse(
        &json!({ "manifestVersion": 1, "appId": "shop", "name": "Shop", "tools": tools })
            .to_string(),
    )
    .expect("清单");
    crate::schema::build(&manifest, None, None)
}

fn generate_all(m: &Model) -> (Vec<GeneratedFile>, Vec<Warning>) {
    let mut w = Vec::new();
    (generate(m, DEFAULT_DOMAIN, DEFAULT_ABILITY, false, &mut w), w)
}

#[test]
fn execute_mode_follows_risk_and_activation() {
    let m = model(json!([
        { "name": "a.read", "description": "读", "inputSchema": { "type": "object" }, "risk": "read" },
        { "name": "a.pay", "description": "付", "inputSchema": { "type": "object" }, "risk": "payment" },
        { "name": "a.show", "description": "显示", "inputSchema": { "type": "object" }, "risk": "read", "activation": "foreground" }
    ]));
    let modes: Vec<&str> = m.tools.iter().map(execute_mode).collect();
    assert_eq!(
        modes,
        [
            "insightIntent.ExecuteMode.UI_ABILITY_BACKGROUND",
            "insightIntent.ExecuteMode.UI_ABILITY_FOREGROUND",
            "insightIntent.ExecuteMode.UI_ABILITY_FOREGROUND"
        ]
    );
    assert!(needs_confirmation(Risk::Destructive));
    assert!(!needs_confirmation(Risk::Write));
}

#[test]
fn skips_tools_whose_params_cannot_be_executor_properties() {
    let m = model(json!([
        { "name": "a.dash", "description": "x", "inputSchema": { "type": "object", "properties": { "is-x": { "type": "boolean" } } } },
        { "name": "a.ctx", "description": "x", "inputSchema": { "type": "object", "properties": { "context": { "type": "string" } } } },
        { "name": "a.ok", "description": "x", "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } } } }
    ]));
    let (files, warnings) = generate_all(&m);
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    let executors: Vec<String> = files
        .iter()
        .filter(|f| f.path.starts_with("ets/insightintents"))
        .map(|f| f.path.display().to_string())
        .collect();
    assert_eq!(executors, ["ets/insightintents/ShopAOkIntent.ets"]);
    let json = files
        .iter()
        .find(|f| f.path.ends_with("insight_intent.json"))
        .expect("配置");
    assert!(
        json.contents
            .contains("./ets/insightintents/ShopAOkIntent.ets")
    );
    assert!(!json.contents.contains("ADash"));
    // 不能生成意图的工具仍注册为 MCP 工具
    let tools = &files[0].contents;
    assert!(tools.contains("\"a.dash\"") && tools.contains("\"is-x\"?: boolean;"));
}

#[test]
fn parameters_drop_format_and_degraded_constructs() {
    let m = model(json!([{
        "name": "a.b",
        "description": "x",
        "inputSchema": {
            "type": "object",
            "properties": {
                "when": { "type": "string", "format": "date-time", "description": "时间" },
                "n": { "type": "integer", "minimum": 1, "default": 2 },
                "raw": { "$ref": "#/definitions/x" },
                "tags": { "type": "array", "items": { "type": "string" }, "minItems": 1 }
            },
            "required": ["n"]
        }
    }]));
    let schema = intent_parameters(&m, m.params(&m.tools[0]));
    assert_eq!(
        schema,
        json!({
            "type": "object",
            "description": "x",
            "properties": {
                "when": { "type": "string", "description": "时间" },
                "n": { "type": "number", "description": "整数", "minimum": 1, "default": 2 },
                "raw": { "type": "string", "description": "JSON 文本" },
                "tags": { "type": "array", "items": { "type": "string" }, "minItems": 1 }
            },
            "required": ["n"]
        })
    );
}

#[test]
fn executor_checks_required_and_passes_optional() {
    let m = model(json!([{
        "name": "cart.add",
        "title": "加入购物车",
        "description": "把商品加入购物车",
        "inputSchema": {
            "type": "object",
            "properties": { "productId": { "type": "string" }, "note": { "type": ["string", "null"] } },
            "required": ["productId"]
        }
    }]));
    let (files, _) = generate_all(&m);
    let exec = files
        .iter()
        .find(|f| f.path.ends_with("ShopCartAddIntent.ets"))
        .expect("执行器");
    let text = &exec.contents;
    assert!(text.contains("intentName: \"ShopCartAdd\","));
    assert!(text.contains("domain: \"ToolsDomain\","));
    assert!(text.contains("keywords: [\"加入购物车\", \"cart.add\"],"));
    assert!(text.contains("public productId?: string;"));
    assert!(text.contains("public note?: string | null;"));
    assert!(text.contains("if (productIdValue === undefined) {"));
    assert!(text.contains("return ShopIntentRuntime.missing(\"productId\");"));
    assert!(text.contains("note: this.note,"));
    assert!(text.contains("handlers.cartAdd(params)"));
}

#[test]
fn custom_domain_and_ability() {
    let m = model(
        json!([{ "name": "a", "description": "x", "inputSchema": { "type": "object" } }]),
    );
    let mut w = Vec::new();
    let files = generate(&m, "ShoppingPlatformsDomain", "MainAbility", false, &mut w);
    let exec = files
        .iter()
        .find(|f| f.path.ends_with("ShopAIntent.ets"))
        .expect("执行器");
    assert!(
        exec.contents
            .contains("domain: \"ShoppingPlatformsDomain\",")
    );
    assert!(exec.contents.contains("abilityName: \"MainAbility\","));
    assert!(exec.contents.contains("const params: AParams = {};"));
}

fn executor_text(m: &Model, file: &str) -> String {
    let (files, _) = generate_all(m);
    files
        .into_iter()
        .find(|f| f.path.ends_with(file))
        .expect("执行器")
        .contents
}

fn one_tool(properties: Value, required: Value) -> Model {
    model(json!([{
        "name": "a.b",
        "description": "x",
        "inputSchema": { "type": "object", "properties": properties, "required": required }
    }]))
}

// ets-loader 的 typeToString 永远不会是 `integer`：顶层整数在 parameters 中为 number，执行时校验整数。
#[test]
fn integer_param_is_number_checked_at_execution() {
    let m = one_tool(
        json!({ "n": { "type": "integer", "description": "数量", "minimum": 1 } }),
        json!(["n"]),
    );
    let schema = intent_parameters(&m, m.params(&m.tools[0]));
    assert_eq!(
        schema["properties"]["n"],
        json!({ "type": "number", "description": "数量（整数）", "minimum": 1 })
    );
    let text = executor_text(&m, "ShopABIntent.ets");
    assert!(text.contains("public n?: number;"), "{text}");
    assert!(text.contains(
        "if (typeof nValue === 'number' && !Number.isInteger(nValue)) {\n      return ShopIntentRuntime.invalid(\"n\", \"须为整数\");"
    ));
    assert!(text.contains("n: nValue,"));
}

// 类型别名的 typeToString 是别名名：枚举属性声明为 string，执行时校验取值再转换为枚举类型。
#[test]
fn enum_param_is_string_converted_at_execution() {
    let m = one_tool(
        json!({ "e": { "enum": ["x", "y"] }, "en": { "enum": ["x", null] } }),
        json!([]),
    );
    let schema = intent_parameters(&m, m.params(&m.tools[0]));
    assert_eq!(schema["properties"]["e"], json!({ "type": "string", "enum": ["x", "y"] }));
    let text = executor_text(&m, "ShopABIntent.ets");
    assert!(text.contains("public e?: string;"), "{text}");
    assert!(text.contains("public en?: string | null;"));
    assert!(text.contains("if (typeof eValue === 'string' && ![\"x\", \"y\"].includes(eValue)) {"));
    assert!(text.contains("return ShopIntentRuntime.invalid(\"e\", \"取值须为 x / y\");"));
    assert!(text.contains("e: eValue === undefined ? eValue : eValue as ABE,"));
    assert!(text.contains("en: enValue === undefined || enValue === null ? enValue : enValue as ABEn,"));
}

// schema 为 object 的顶层属性须是 @InsightIntentEntity 类；嵌套对象在实体内保持参数接口类型。
#[test]
fn object_param_is_entity_converted_at_execution() {
    let m = one_tool(
        json!({ "o": {
            "type": "object",
            "description": "选项",
            "properties": {
                "k": { "type": "string" },
                "q": { "type": "integer" },
                "sub": { "type": "object", "properties": { "z": { "type": "boolean" } } }
            },
            "required": ["k"]
        } }),
        json!(["o"]),
    );
    let schema = intent_parameters(&m, m.params(&m.tools[0]));
    assert_eq!(schema["properties"]["o"]["type"], json!("object"));
    assert_eq!(
        schema["properties"]["o"]["properties"]["q"],
        json!({ "type": "integer" }),
        "嵌套位置不受顶层表示影响"
    );
    let text = executor_text(&m, "ShopABIntent.ets");
    assert!(text.contains(
        "import { insightIntent, InsightIntentEntity, InsightIntentEntry, InsightIntentEntryExecutor } from '@kit.AbilityKit';"
    ), "{text}");
    assert!(text.contains("import { ABO, ABOSub, ABParams, ShopToolHandlers } from '../appmcp/ShopTools';"));
    assert!(text.contains("@InsightIntentEntity({\n  entityCategory: \"ShopAB.o\",\n})"));
    assert!(text.contains("export class ShopABOEntity implements insightIntent.IntentEntity {\n  public entityId: string = '';"));
    assert!(text.contains("  public q?: number;\n  public sub?: ABOSub;\n}"));
    assert!(text.contains("function toABO(entity: ShopABOEntity): ABO | undefined {"));
    assert!(text.contains("  const kValue = entity.k;\n  if (kValue === undefined) {\n    return undefined;\n  }"));
    assert!(text.contains("public o?: ShopABOEntity;"));
    assert!(text.contains("const oValue = toABO(oEntity);\n    if (oValue === undefined) {"));
    assert!(text.contains("return ShopIntentRuntime.invalid(\"o\", \"缺少必填属性 k\");"));
    assert!(text.contains("o: oValue,"));
}

// 字典与原始 JSON 在 parameters 中为字符串（JSON 文本），执行时校验并解析。
#[test]
fn map_and_json_params_are_json_text() {
    let m = one_tool(
        json!({
            "m": { "type": "object", "additionalProperties": { "type": "integer" }, "description": "标签" },
            "j": { "description": "任意" }
        }),
        json!(["j"]),
    );
    let schema = intent_parameters(&m, m.params(&m.tools[0]));
    assert_eq!(schema["properties"]["m"], json!({ "type": "string", "description": "标签（JSON 对象文本）" }));
    assert_eq!(schema["properties"]["j"], json!({ "type": "string", "description": "任意（JSON 文本）" }));
    let text = executor_text(&m, "ShopABIntent.ets");
    assert!(text.contains("public m?: string;") && text.contains("public j?: string;"), "{text}");
    assert!(text.contains("if (typeof mText === 'string' && !ShopIntentRuntime.isJsonObject(mText, false)) {"));
    assert!(text.contains("if (typeof jText === 'string' && !ShopIntentRuntime.isJson(jText)) {"));
    assert!(text.contains("m: mText === undefined ? mText : ShopIntentRuntime.parseJson(mText) as Record<string, number>,"));
    assert!(text.contains("j: ShopIntentRuntime.parseJson(jText),"));
}

// 非实体数组不参与构建时的类型校验：数组属性保持参数类型，原样传递。
#[test]
fn array_param_keeps_tool_type() {
    let m = one_tool(
        json!({ "a": { "type": "array", "items": { "enum": ["p", "q"] } } }),
        json!([]),
    );
    let schema = intent_parameters(&m, m.params(&m.tools[0]));
    assert_eq!(
        schema["properties"]["a"],
        json!({ "type": "array", "items": { "type": "string", "enum": ["p", "q"] } })
    );
    let text = executor_text(&m, "ShopABIntent.ets");
    assert!(text.contains("public a?: ABAItem[];"), "{text}");
    assert!(text.contains("a: this.a,"));
}

#[test]
fn skips_object_params_that_cannot_be_entities() {
    let m = model(json!([
        { "name": "a.id", "description": "x", "inputSchema": { "type": "object", "properties": {
            "o": { "type": "object", "properties": { "entityId": { "type": "string" } } } } } },
        { "name": "a.dash", "description": "x", "inputSchema": { "type": "object", "properties": {
            "o": { "type": "object", "properties": { "is-x": { "type": "string" } } } } } }
    ]));
    let (files, warnings) = generate_all(&m);
    let messages: Vec<&str> = warnings.iter().map(|w| w.message.as_str()).collect();
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert!(messages[0].contains("`entityId` 与 IntentEntity 的成员同名"));
    assert!(messages[1].contains("`is-x` 不是标识符"));
    assert!(!files.iter().any(|f| f.path.starts_with("ets/insightintents")));
}

#[test]
fn runtime_reports_invalid_input() {
    let m = one_tool(json!({}), json!([]));
    let (files, _) = generate_all(&m);
    let runtime = &files[1].contents;
    assert!(runtime.contains("static invalid(field: string, reason: string): Promise<insightIntent.IntentResult<string>> {"));
    assert!(runtime.contains("ShopIntentCode.INVALID_INPUT, 'INVALID_INPUT', `参数 ${field} 无效：${reason}`"));
    assert!(runtime.contains("return typeof value === 'object' && !Array.isArray(value);"));
}
