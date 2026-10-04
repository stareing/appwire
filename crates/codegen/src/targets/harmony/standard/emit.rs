//! 鸿蒙标准意图的 ArkTS 输出：执行器文件与媒体实体解析文件。

use super::super::entity::{EntityProp, entity_class};
use super::super::executor::display_description;
use super::*;

fn lit(s: &str) -> String {
    string_literal(Lang::TypeScript, s)
}

/// 媒体请求接口名：`<Module><标准意图>Request`。
fn request_name(model: &Model, intent: &StandardIntent) -> String {
    format!("{}{}Request", model.module, intent.name)
}

/// 解析器方法名：标准意图名首字母小写（`playVideo`）。
fn resolver_method(intent: &StandardIntent) -> String {
    let mut chars = intent.name.chars();
    chars
        .next()
        .map(|c| c.to_ascii_lowercase().to_string() + chars.as_str())
        .unwrap_or_default()
}

/// 导航意图的位置实体类名。
fn location_class(model: &Model) -> String {
    format!("{}StandardNavigateLocation", model.module)
}

fn prop_ty(model: &Model, p: &Prop) -> String {
    if p.ty == ENTITY_TYPE {
        location_class(model)
    } else {
        p.ty.to_string()
    }
}

pub(super) fn media_file(model: &Model, plans: &[Plan]) -> String {
    let m = &model.module;
    let media: Vec<&Plan> = plans.iter().filter(|p| p.navigation.is_none()).collect();
    let mut c = Code::new("  ");
    let mut header = header_lines(model, "harmony-insight-intents");
    header.extend([
        String::new(),
        "媒体标准意图（--standard-intents）的实体解析：系统入口只传 App 侧实体 ID，由 App 解析为工具参数，".to_string(),
        format!("执行器随后调用 {m}ToolHandlers 的同一方法。在 AbilityStage.onCreate 中设置 `{m}StandardIntents.mediaResolver`。"),
    ]);
    c.comment("// ", &header);
    c.blank();
    c.line(format!(
        "import {{ ToolCallError }} from {};",
        lit(SDK_PACKAGE)
    ));
    let params: BTreeSet<String> = media
        .iter()
        .map(|p| model.params(p.tool).name.clone())
        .collect();
    c.line(format!(
        "import {{ {} }} from './{m}Tools';",
        params.into_iter().collect::<Vec<_>>().join(", ")
    ));
    c.blank();

    for plan in &media {
        c.line(format!(
            "/** {} 的请求（系统入口按标准意图 schema 赋值）。 */",
            plan.intent.name
        ));
        c.open(format!(
            "export interface {} {{",
            request_name(model, &plan.intent)
        ));
        for p in plan.intent.props {
            c.line(format!("/** {} */", p.doc));
            let q = if p.required { "" } else { "?" };
            c.line(format!("{}{q}: {};", p.name, prop_ty(model, p)));
        }
        c.close("}");
        c.blank();
    }

    c.block_doc(&[
        "媒体标准意图的实体解析：把系统传入的实体 ID 转换为工具参数（实体 ID 是 App 提供给系统的内容 ID）。".to_string(),
        "@error 无法解析时抛出 ToolCallError（如 RESOURCE_NOT_FOUND），意图返回失败结果。".to_string(),
    ]);
    c.open(format!("export interface {m}MediaEntityResolver {{"));
    for plan in &media {
        let params = &model.params(plan.tool).name;
        c.line(format!(
            "/** {} → 工具 `{}`。 */",
            plan.intent.name, plan.tool.info.name
        ));
        c.line(format!(
            "{}(request: {}): {params} | Promise<{params}>;",
            resolver_method(&plan.intent),
            request_name(model, &plan.intent)
        ));
    }
    c.close("}");
    c.blank();

    c.open(format!("export class {m}StandardIntents {{"));
    c.line("/** 由 App 在启动时设置；未设置时媒体标准意图返回失败结果（HANDLER_ERROR）。 */");
    c.line(format!(
        "static mediaResolver: {m}MediaEntityResolver | undefined = undefined;"
    ));
    c.blank();
    c.line("/** 取解析器；未设置时为失败的 Promise。 */");
    c.open(format!(
        "static media(): Promise<{m}MediaEntityResolver> {{"
    ));
    c.line(format!(
        "const resolver = {m}StandardIntents.mediaResolver;"
    ));
    c.open("if (resolver === undefined) {");
    c.line(format!(
        "return Promise.reject(new ToolCallError('HANDLER_ERROR', '未设置 {m}StandardIntents.mediaResolver'));"
    ));
    c.close("}");
    c.line("return Promise.resolve(resolver);");
    c.close("}");
    c.close("}");
    c.finish()
}

pub(super) fn executor_file(model: &Model, plan: &Plan, ability: &str) -> String {
    let m = &model.module;
    let tool = plan.tool;
    let intent = &plan.intent;
    let params = &model.params(tool).name;
    let class = executor_class(model, intent);

    let mut c = Code::new("  ");
    let mut header = header_lines(model, "harmony-insight-intents");
    header.push(format!(
        "工具 `{}` 的鸿蒙标准意图 {}（--standard-intents；insight_intent.json 的 insightIntentsSrcEntry 引用本文件）。",
        tool.info.name, intent.name
    ));
    c.comment("// ", &header);
    c.blank();
    let mut tool_imports = BTreeSet::from([params.clone(), format!("{m}ToolHandlers")]);
    if let Some(nav) = &plan.navigation {
        c.line("import { insightIntent, InsightIntentEntity, InsightIntentEntry, InsightIntentEntryExecutor } from '@kit.AbilityKit';");
        tool_imports.insert(nav.destination_decl.name.clone());
        if let Some((f, _)) = &nav.mode {
            tool_imports.extend(mode_import(model, f));
        }
    } else {
        c.line("import { insightIntent, InsightIntentEntry, InsightIntentEntryExecutor } from '@kit.AbilityKit';");
    }
    c.line(format!(
        "import {{ {m}IntentRuntime }} from '../../appmcp/{m}InsightIntents';"
    ));
    if plan.navigation.is_none() {
        c.line(format!(
            "import {{ {m}MediaEntityResolver, {}, {m}StandardIntents }} from '../../appmcp/{m}StandardIntents';",
            request_name(model, intent)
        ));
    }
    c.line(format!(
        "import {{ {} }} from '../../appmcp/{m}Tools';",
        tool_imports.into_iter().collect::<Vec<_>>().join(", ")
    ));
    c.blank();
    if let Some(nav) = &plan.navigation {
        navigation_helpers(&mut c, model, nav);
    }

    let mut doc = vec![format!(
        "工具 `{}` 的鸿蒙标准意图 {}（{}）。",
        tool.info.name,
        intent.name,
        plan.verb.id()
    )];
    doc.push(if plan.navigation.is_some() {
        "目的地由 dstLocation 转换（坐标只在坐标系为 WGS-84 时传递），交通方式由 trafficType 转换。"
            .to_string()
    } else {
        format!("系统传入 App 实体 ID，经 {m}StandardIntents.mediaResolver 解析为工具参数。")
    });
    if needs_confirmation(tool.info.risk) {
        doc.push(format!(
            "风险 {}：意图框架没有系统级确认，以前台模式执行，handler 应在执行前向用户确认。",
            risk_name(tool.info.risk)
        ));
    }
    c.block_doc(&doc);
    c.open("@InsightIntentEntry({");
    c.line(format!("intentName: {},", lit(intent.name)));
    c.line(format!("domain: {},", lit(intent.domain)));
    c.line(format!("intentVersion: {},", lit(intent.version)));
    c.line(format!("displayName: {},", lit(tool.display_title())));
    c.line(format!(
        "displayDescription: {},",
        lit(display_description(tool))
    ));
    c.line(format!("schema: {},", lit(intent.name)));
    c.line(format!("abilityName: {},", lit(ability)));
    c.line(format!("executeMode: [{}],", execute_mode(tool)));
    c.close("})");
    c.open(format!(
        "export default class {class} extends InsightIntentEntryExecutor<string> {{"
    ));
    for p in intent.props {
        c.line(format!("/** {} */", p.doc));
        c.line(format!("public {}?: {};", p.name, prop_ty(model, p)));
    }
    c.blank();
    c.open("onExecute(): Promise<insightIntent.IntentResult<string>> {");
    match &plan.navigation {
        Some(nav) => navigation_body(&mut c, model, plan, nav),
        None => media_body(&mut c, model, plan),
    }
    c.close("}");
    c.close("}");
    c.finish()
}

fn mode_import(model: &Model, f: &Field) -> Option<String> {
    match f.ty {
        Ty::Enum(id) => Some(model.decl(id).name().to_string()),
        _ => None,
    }
}

fn media_body(c: &mut Code, model: &Model, plan: &Plan) {
    let m = &model.module;
    let intent = &plan.intent;
    let mut values = Vec::new();
    for p in intent.props {
        if p.required {
            let local = format!("{}Value", p.name);
            c.line(format!("const {local} = this.{};", p.name));
            c.open(format!("if ({local} === undefined) {{"));
            c.line(format!("return {m}IntentRuntime.missing({});", lit(p.name)));
            c.close("}");
            values.push(format!("{}: {local}", p.name));
        } else {
            values.push(format!("{0}: this.{0}", p.name));
        }
    }
    c.open(format!(
        "const request: {} = {{",
        request_name(model, intent)
    ));
    for v in &values {
        c.line(format!("{v},"));
    }
    c.close("};");
    c.line(format!(
        "return {m}IntentRuntime.run((handlers: {m}ToolHandlers) => {m}StandardIntents.media()"
    ));
    c.line(format!(
        "  .then((resolver: {m}MediaEntityResolver) => resolver.{}(request))",
        resolver_method(intent)
    ));
    c.line(format!(
        "  .then((params: {}) => handlers.{}(params)));",
        model.params(plan.tool).name,
        plan.tool.camel
    ));
}

/// 位置实体类、交通方式表与坐标转换函数（导航执行器文件内）。
fn navigation_helpers(c: &mut Code, model: &Model, nav: &NavigationPlan) {
    let prop = |name: &str, doc: &[&str]| {
        EntityProp::new(name, "string", doc.iter().map(|d| d.to_string()).collect())
    };
    entity_class(
        c,
        &["StartNavigate 的地点（系统入口按标准意图 schema 赋值；字段均为字符串）。".to_string()],
        "location",
        &location_class(model),
        &[
            prop("poiId", &[]),
            prop("locationName", &[]),
            prop("locationSystem", &["坐标系，缺省为 GCJ-02。"]),
            prop("longitude", &[]),
            prop("latitude", &[]),
            prop("address", &[]),
        ],
    );
    c.blank();
    if let Some((f, table)) = &nav.mode {
        let ty = mode_import(model, f).unwrap_or_else(|| "string".to_string());
        c.line("/** trafficType → mode。 */");
        c.open(format!("const TRAFFIC_MODES: Record<string, {ty}> = {{"));
        for (traffic, mode) in table {
            c.line(format!("{}: {},", lit(traffic), lit(mode)));
        }
        c.close("};");
        c.blank();
    }
    if nav.fields.iter().any(|f| f.source.numeric) {
        c.block_doc(&[
            "坐标字符串转为数值；只在坐标系明确为 WGS-84 时传递，否则为 undefined。".to_string(),
            "@why 标准意图坐标缺省为 GCJ-02，词表 destination 的 lat / lng 按 WGS-84 解释；不做坐标转换。".to_string(),
        ]);
        c.open("function wgs84Coordinate(system: string | undefined, value: string | undefined): number | undefined {");
        c.open("if (value === undefined || system === undefined || system.replace('-', '').toUpperCase() !== 'WGS84') {");
        c.line("return undefined;");
        c.close("}");
        c.line("const n = Number(value);");
        c.line("return Number.isFinite(n) ? n : undefined;");
        c.close("}");
        c.blank();
    }
}

fn navigation_body(c: &mut Code, model: &Model, plan: &Plan, nav: &NavigationPlan) {
    let m = &model.module;
    c.line("const dst = this.dstLocation;");
    c.open("if (dst === undefined) {");
    c.line(format!("return {m}IntentRuntime.missing(\"dstLocation\");"));
    c.close("}");
    for f in &nav.fields {
        let s = f.source;
        let value = if s.numeric {
            format!("wgs84Coordinate(dst.locationSystem, dst.{})", s.source)
        } else {
            format!("dst.{}", s.source)
        };
        c.line(format!("const {}Value = {value};", s.target));
    }
    for f in nav.fields.iter().filter(|f| f.field.required) {
        c.open(format!("if ({}Value === undefined) {{", f.source.target));
        c.line(format!(
            "return {m}IntentRuntime.missing({});",
            lit(&format!("dstLocation.{}", f.source.source))
        ));
        c.close("}");
    }
    let has = |t: &str| nav.fields.iter().any(|f| f.source.target == t);
    let mut empty = Vec::new();
    for t in ["name", "address"].into_iter().filter(|t| has(t)) {
        empty.push(format!("{t}Value === undefined"));
    }
    if has("lat") && has("lng") {
        empty.push("(latValue === undefined || lngValue === undefined)".to_string());
    }
    c.line("// 词表要求 name / address / lat + lng 至少一组");
    c.open(format!("if ({}) {{", empty.join(" && ")));
    c.line(format!("return {m}IntentRuntime.missing(\"dstLocation\");"));
    c.close("}");
    c.open(format!(
        "const destination: {} = {{",
        nav.destination_decl.name
    ));
    for f in &nav.fields {
        c.line(format!("{0}: {0}Value,", f.source.target));
    }
    c.close("};");
    c.open(format!(
        "const params: {} = {{",
        model.params(plan.tool).name
    ));
    c.line(format!("{}: destination,", nav.destination.json_name));
    if let Some((f, _)) = &nav.mode {
        c.line(format!(
            "{}: this.trafficType === undefined ? undefined : TRAFFIC_MODES[this.trafficType],",
            f.json_name
        ));
    }
    c.close("};");
    c.line(format!(
        "return {m}IntentRuntime.run((handlers: {m}ToolHandlers) => handlers.{}(params));",
        plan.tool.camel
    ));
}
