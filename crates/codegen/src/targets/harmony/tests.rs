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
                "n": { "type": "integer", "minimum": 1, "default": 2 },
                "raw": {},
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
