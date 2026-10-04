//! `--standard-intents` 的 Android 输出（spec/intents.md 第 3 节）：为声明了 `implements` 的工具生成系统意图版本。
//!
//! 输出两个文件（只在有绑定时）：
//! - `<Module>StandardIntents.kt`：把 Activity 收到的 `android.content.Intent` 解析为工具调用
//!   （`<Module>StandardIntents.parse` → `Matched` / `Invalid` / `Unrecognized`），调用复用 `<Module>ToolHandlers`；
//! - `<Module>StandardIntentFilters.xml`：`<activity>` 片段，内含各动词的 `<intent-filter>`，由 App 复制进自己的 Activity。
//!
//! 映射表（action、data scheme / MIME、参数来源）只在 [`table`] 中定义。

mod table;
#[cfg(test)]
mod tests;

use app_mcp_protocol::intents::IntentDef;

use crate::GeneratedFile;
use crate::code::{Code, header_lines, string_literal, xml_escape};
use crate::ident::{self, Lang, NameScope};
use crate::schema::{Field, Model, ToolModel, Ty, Warning};
use crate::standard_intents::{self, Binding};
use crate::targets::{file, kotlin, risk_name};

use table::{AndroidIntent, Filter, PLACE_FIELDS, Param, Value};

/// 一个已确认可生成的绑定：工具、动词与各参数的取值规则。
struct Bound<'m> {
    tool: &'m ToolModel,
    intent_id: String,
    android: &'static AndroidIntent,
    args: Vec<ArgSpec>,
}

/// 工具参数的取值规则（对应 Kotlin `Arg`）。
#[derive(Debug, PartialEq)]
struct ArgSpec {
    name: String,
    required: bool,
    /// 工具参数为枚举时允许的取值。
    allowed: Option<Vec<String>>,
    /// 工具参数为对象时保留的键。
    keys: Option<Vec<String>>,
}

pub fn generate(model: &Model, package: &str, warnings: &mut Vec<Warning>) -> Vec<GeneratedFile> {
    let lookup = |def: &IntentDef| table::lookup(def.verb, def.version);
    let bound: Vec<Bound> =
        standard_intents::bindings(model, "Android", lookup, |b, w| resolve(model, b, w), warnings);
    if bound.is_empty() {
        return Vec::new();
    }
    let m = &model.module;
    vec![
        file(format!("{m}StandardIntents.kt"), kotlin_file(model, package, &bound)),
        file(format!("{m}StandardIntentFilters.xml"), manifest_file(model, &bound)),
    ]
}

/// 对照工具的参数类型确定各参数的取值规则；工具的必填参数无法从 Intent 得到时不生成（警告）。
fn resolve<'m>(
    model: &'m Model,
    b: Binding<'m, &'static AndroidIntent>,
    warnings: &mut Vec<Warning>,
) -> Option<Bound<'m>> {
    let params = model.params(b.tool);
    let mut warn = |path: &str, message: String| {
        warnings.push(Warning { tool: b.tool.info.name.clone(), path: path.to_string(), message });
    };
    let mut args = Vec::new();
    for p in b.system.params {
        let Some(field) = params.fields.iter().find(|f| f.json_name == p.name) else { continue };
        match arg_spec(model, p, field) {
            Some(arg) => args.push(arg),
            None if field.required => {
                warn(p.name, format!("类型与 Android 系统意图的取值（{}）不符，不生成系统意图", p.source));
                return None;
            }
            None => warn(p.name, format!("类型与 Android 系统意图的取值（{}）不符，系统意图不传此参数", p.source)),
        }
    }
    if let Some(f) = params.fields.iter().find(|f| f.required && !args.iter().any(|a| a.name == f.json_name)) {
        warn(&f.json_name, format!("必填参数无法从 Android 系统意图（{}）得到，不生成系统意图", b.intent.id()));
        return None;
    }
    Some(Bound { tool: b.tool, intent_id: b.intent.id(), android: b.system, args })
}

fn arg_spec(model: &Model, p: &Param, field: &Field) -> Option<ArgSpec> {
    let mut arg = ArgSpec { name: field.json_name.clone(), required: field.required, allowed: None, keys: None };
    match (p.value, &field.ty) {
        (Value::Str, Ty::Enum(id)) => arg.allowed = Some(model.enum_decl(*id)?.values.clone()),
        (Value::Place, Ty::Object(id)) => {
            let o = model.object(*id)?;
            let keys: Vec<String> = PLACE_FIELDS
                .iter()
                .filter(|(k, v)| o.fields.iter().any(|f| f.json_name == *k && accepts(*v, &f.ty)))
                .map(|(k, _)| k.to_string())
                .collect();
            if keys.is_empty() {
                return None;
            }
            arg.keys = Some(keys);
        }
        (v, t) if accepts(v, t) => {}
        _ => return None,
    }
    Some(arg)
}

/// Intent 中取到的值能否解码为该类型。
fn accepts(value: Value, ty: &Ty) -> bool {
    match (value, ty) {
        (_, Ty::Json) => true,
        (Value::Str, Ty::String | Ty::Enum(_)) | (Value::Bool, Ty::Boolean) | (Value::Number, Ty::Number) => true,
        (Value::StrList, Ty::Array(inner)) => matches!(**inner, Ty::String | Ty::Json),
        (Value::Place, Ty::Object(_)) => true,
        (Value::Place, Ty::Map(inner)) => matches!(**inner, Ty::Json),
        _ => false,
    }
}

struct Names {
    intents: String,
    call: String,
    result: String,
    support: String,
}

fn names(model: &Model) -> Names {
    let m = &model.module;
    Names {
        intents: format!("{m}StandardIntents"),
        call: format!("{m}StandardIntentCall"),
        result: format!("{m}StandardIntentResult"),
        support: format!("{m}StandardIntentSupport"),
    }
}

fn kotlin_file(model: &Model, package: &str, bound: &[Bound]) -> String {
    let m = &model.module;
    let n = names(model);
    let mut c = Code::new("    ");
    let mut header = header_lines(model, "kotlin-appfunctions");
    header.extend([
        String::new(),
        "Android 系统意图（spec/intents.md 第 3 节，--standard-intents）：把系统 Intent 解析为工具调用，".to_string(),
        format!("handler 与 MCP / AppFunctions 共用 {m}ToolHandlers。依赖同目录下的 {m}Tools.kt；"),
        format!("intent-filter 见 {m}StandardIntentFilters.xml。"),
        String::new(),
        "绑定：".to_string(),
    ]);
    for b in bound {
        let actions: Vec<&str> = b.android.filters.iter().map(|f| short_action(f.action)).collect();
        header.push(format!("  {} → {}（{}）", b.intent_id, b.tool.info.name, dedup(actions).join("、")));
    }
    header.extend([
        String::new(),
        "在处理这些意图的 Activity 的 onCreate 与 onNewIntent 中：".to_string(),
        format!("  when (val r = {}.parse(intent)) {{", n.intents),
        format!("      is {}.Matched -> lifecycleScope.launch {{ r.call.call(handlers) }}", n.result),
        format!("      is {}.Invalid -> {{ /* 显示 r.reason 或回到普通界面 */ }}", n.result),
        format!("      {}.Unrecognized -> {{ /* 普通启动 */ }}", n.result),
        "  }".to_string(),
        String::new(),
        "@security Intent 可来自任意 App（导出的 Activity 不鉴权调用方）：参数不可信；有副作用的工具（r.call.risk）".to_string(),
        "由 App 在调用前向用户确认。content: URI 的读权限随 Intent 授予本 Activity，handler 应在 Activity 结束前读取。".to_string(),
    ]);
    if bound.iter().any(|b| b.android.verb == "calendar.create") {
        header.push("calendar.create 使用 java.time（API 26+，更低版本需开启 core library desugaring）。".to_string());
    }
    c.comment("// ", &header);
    c.line("@file:Suppress(\"unused\", \"RedundantVisibilityModifier\")");
    c.blank();
    c.line(format!("package {package}"));
    c.blank();
    for i in imports(bound) {
        c.line(i);
    }
    c.blank();

    let classes = emit_call(&mut c, model, &n, bound);
    c.blank();
    emit_result(&mut c, &n);
    c.blank();
    emit_parse(&mut c, model, &n, bound, &classes);
    c.blank();
    emit_support(&mut c, &n, bound);
    c.finish()
}

fn short_action(action: &str) -> &str {
    action.rsplit('.').next().unwrap_or(action)
}

fn dedup(items: Vec<&str>) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    for i in items {
        if !out.contains(&i) {
            out.push(i);
        }
    }
    out
}

fn imports(bound: &[Bound]) -> Vec<&'static str> {
    let mut all: Vec<&'static str> = table::SUPPORT_IMPORTS.to_vec();
    all.extend(bound.iter().flat_map(|b| b.android.imports.iter().copied()));
    all.sort_unstable();
    all.dedup();
    all
}

/// 每个绑定一个调用类；返回各类名（与 `bound` 对应）。
fn emit_call(c: &mut Code, model: &Model, n: &Names, bound: &[Bound]) -> Vec<String> {
    let m = &model.module;
    let handlers = format!("{m}ToolHandlers");
    // @why 调用类嵌套在接口内，类名不能遮蔽类体里引用的类型（参数类型、handler 接口、String、JsonElement）
    let mut reserved: Vec<String> = vec!["String".into(), "JsonElement".into(), handlers.clone()];
    reserved.extend(bound.iter().map(|b| model.params(b.tool).name.clone()));
    let reserved: Vec<&str> = reserved.iter().map(String::as_str).collect();
    let mut scope = NameScope::with_reserved(&reserved);

    c.block_doc(&["标准意图解析出的工具调用（参数已解码为工具参数类型）。".to_string()]);
    c.open(format!("sealed interface {} {{", n.call));
    c.line("/** 工具全名。 */");
    c.line("val tool: String");
    c.line("/** 清单声明的风险（read / write / destructive / payment / os-sensitive），供 App 决定是否先确认。 */");
    c.line("val risk: String");
    c.line(format!("/** 调用 handler（与 MCP、AppFunctions 共用 {handlers}）。 */"));
    c.line(format!("suspend fun call(handlers: {handlers}): JsonElement"));
    let mut classes = Vec::new();
    for b in bound {
        let class = scope.claim(&b.tool.pascal);
        let title = b.tool.info.title.as_deref().map(|t| format!("「{t}」")).unwrap_or_default();
        c.blank();
        c.line(format!("/** {} → 工具 `{}`{title}。 */", b.intent_id, b.tool.info.name));
        c.open(format!(
            "class {class}(val params: {}) : {} {{",
            model.params(b.tool).name,
            n.call
        ));
        c.line(format!("override val tool: String get() = {}", string_literal(Lang::Kotlin, &b.tool.info.name)));
        c.line(format!("override val risk: String get() = {}", string_literal(Lang::Kotlin, risk_name(b.tool.info.risk))));
        if b.tool.deprecation().is_some() {
            c.comment("// ", &crate::deprecation::tool_doc_lines(b.tool));
            c.line(kotlin::SUPPRESS_DEPRECATION);
        }
        c.line(format!(
            "override suspend fun call(handlers: {handlers}): JsonElement = handlers.{}(params)",
            ident::escape(Lang::Kotlin, &b.tool.camel)
        ));
        c.close("}");
        classes.push(class);
    }
    c.close("}");
    classes
}

fn emit_result(c: &mut Code, n: &Names) {
    c.line(format!("/** {}.parse 的结果。 */", n.intents));
    c.open(format!("sealed interface {} {{", n.result));
    c.line("/** Intent 对应本 App 绑定的一个标准意图。 */");
    c.line(format!("class Matched(val call: {}) : {}", n.call, n.result));
    c.line("/** Intent 对应工具 `tool`，但无法读取、缺少必填参数或参数不合法；`reason` 可记录或提示用户。 */");
    c.line(format!("class Invalid(val tool: String, val reason: String) : {}", n.result));
    c.line("/** 不是本 App 绑定的标准意图（如从桌面启动）。 */");
    c.line(format!("object Unrecognized : {}", n.result));
    c.close("}");
}

fn emit_parse(c: &mut Code, model: &Model, n: &Names, bound: &[Bound], classes: &[String]) {
    let m = &model.module;
    let s = &n.support;
    c.line("/** Android 系统意图 → 工具调用。 */");
    c.open(format!("object {} {{", n.intents));
    c.line("/** 绑定表：标准意图（动词@主版本）→ 工具全名。 */");
    c.open("val bindings: Map<String, String> = mapOf(");
    for b in bound {
        c.line(format!(
            "{} to {},",
            string_literal(Lang::Kotlin, &b.intent_id),
            string_literal(Lang::Kotlin, &b.tool.info.name)
        ));
    }
    c.close(")");
    c.blank();
    c.block_doc(&[
        "把 Activity 收到的 Intent 解析为工具调用；不调用 handler。".to_string(),
        String::new(),
        "ACTION_SEND 带 EXTRA_STREAM 归 file.share，不带归 message.send；对应动词未绑定时为 Unrecognized。".to_string(),
    ]);
    c.open(format!("fun parse(intent: Intent): {} = when {{", n.result));
    for (b, class) in bound.iter().zip(classes) {
        let predicates: Vec<String> = b.android.predicates.iter().map(|p| format!("{s}.{p}(intent)")).collect();
        let args: Vec<String> = b.args.iter().map(|a| kotlin_arg(s, a)).collect();
        c.open(format!("{} -> bind(", predicates.join(" || ")));
        c.line(format!("{},", string_literal(Lang::Kotlin, &b.tool.info.name)));
        c.line(format!("{{ {s}.{}(intent) }},", b.android.extractor));
        c.line(format!("listOf({}),", args.join(", ")));
        c.close(") {");
        c.indent();
        c.line(format!(
            "{}.{class}({m}Tools.json.decodeFromJsonElement({}.serializer(), it))",
            n.call,
            model.params(b.tool).name
        ));
        c.close("}");
    }
    c.line(format!("else -> {}.Unrecognized", n.result));
    c.close("}");
    c.blank();
    c.block_doc(&[
        "提取参数、按工具的参数规则筛选并解码。".to_string(),
        String::new(),
        "@error Intent 无法读取（如 extras 反序列化失败）、缺少必填参数或解码失败时返回 Invalid，不抛出。".to_string(),
    ]);
    c.line("private fun bind(");
    c.indent();
    c.line("tool: String,");
    c.line("extract: () -> Map<String, JsonElement>,");
    c.line(format!("args: List<{s}.Arg>,"));
    c.line(format!("build: (JsonObject) -> {},", n.call));
    c.dedent();
    c.open(format!("): {} {{", n.result));
    c.open("val raw = try {");
    c.line("extract()");
    c.dedent();
    c.line("} catch (e: RuntimeException) {");
    c.indent();
    c.line(format!("return {}.Invalid(tool, \"Intent 无法读取：${{e.message}}\")", n.result));
    c.close("}");
    c.line("val picked = LinkedHashMap<String, JsonElement>()");
    c.open("for (arg in args) {");
    c.line("val value = arg.pick(raw[arg.name])");
    c.open("if (value != null) {");
    c.line("picked[arg.name] = value");
    c.dedent();
    c.line("} else if (arg.required) {");
    c.indent();
    c.line(format!("return {}.Invalid(tool, \"Intent 中缺少参数 ${{arg.name}}\")", n.result));
    c.close("}");
    c.close("}");
    c.open("return try {");
    c.line(format!("{}.Matched(build(JsonObject(picked)))", n.result));
    c.dedent();
    c.line("} catch (e: IllegalArgumentException) {");
    c.indent();
    c.line(format!("{}.Invalid(tool, \"参数不合法：${{e.message}}\")", n.result));
    c.close("}");
    c.close("}");
    c.close("}");
}

fn kotlin_set(values: &[String]) -> String {
    let items: Vec<String> = values.iter().map(|v| string_literal(Lang::Kotlin, v)).collect();
    format!("setOf({})", items.join(", "))
}

fn kotlin_arg(support: &str, a: &ArgSpec) -> String {
    let mut parts = vec![string_literal(Lang::Kotlin, &a.name), format!("required = {}", a.required)];
    if let Some(v) = &a.allowed {
        parts.push(format!("allowed = {}", kotlin_set(v)));
    }
    if let Some(k) = &a.keys {
        parts.push(format!("keys = {}", kotlin_set(k)));
    }
    format!("{support}.Arg({})", parts.join(", "))
}

/// support 对象：共用辅助 + 已绑定动词的谓词与提取函数（每个动词只输出一次）。
fn emit_support(c: &mut Code, n: &Names, bound: &[Bound]) {
    c.line("/** Intent 识别与参数提取（映射表见 app-mcp-codegen 的 appfunctions/standard/table.rs）。 */");
    c.open(format!("internal object {} {{", n.support));
    emit_snippet(c, table::HELPERS);
    let mut done: Vec<&str> = Vec::new();
    for b in bound {
        if done.contains(&b.android.verb) {
            continue;
        }
        done.push(b.android.verb);
        c.blank();
        c.line(format!("// ---- {}", b.android.verb));
        emit_snippet(c, b.android.code);
    }
    c.close("}");
}

fn emit_snippet(c: &mut Code, code: &str) {
    for l in code.lines() {
        c.line(l);
    }
}

fn manifest_file(model: &Model, bound: &[Bound]) -> String {
    let n = names(model);
    let mut header = header_lines(model, "kotlin-appfunctions");
    header.extend([
        String::new(),
        "Android 系统意图的 intent-filter（spec/intents.md 第 3 节，codegen 选项 standard-intents）。".to_string(),
        "把下面 <activity> 中的 <intent-filter> 复制到 AndroidManifest.xml 里处理这些意图的 Activity".to_string(),
        "（须 android:exported=\"true\"），并在其 onCreate / onNewIntent 中调用".to_string(),
        format!("{}.parse(intent)（{}.kt）。", n.intents, n.intents),
    ]);
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!--\n");
    for l in &header {
        out.push_str(&format!("  {}\n", xml_comment(l)).replace("  \n", "\n"));
    }
    out.push_str("-->\n");
    out.push_str("<activity xmlns:android=\"http://schemas.android.com/apk/res/android\" android:exported=\"true\">\n");
    for b in bound {
        out.push_str(&format!("    <!-- {} → 工具 {} -->\n", b.intent_id, xml_comment(&b.tool.info.name)));
        for f in b.android.filters {
            emit_filter(&mut out, f);
        }
    }
    out.push_str("</activity>\n");
    out
}

/// XML 注释内不得出现 `--`（`<`、`&` 在注释内无需转义）。
fn xml_comment(text: &str) -> String {
    let mut s = text.to_string();
    while s.contains("--") {
        s = s.replace("--", "- -");
    }
    s
}

fn emit_filter(out: &mut String, f: &Filter) {
    let attr = |s: &str| xml_escape(s).replace('"', "&quot;");
    out.push_str("    <intent-filter>\n");
    out.push_str(&format!("        <action android:name=\"{}\" />\n", attr(f.action)));
    out.push_str("        <category android:name=\"android.intent.category.DEFAULT\" />\n");
    if f.browsable {
        out.push_str("        <category android:name=\"android.intent.category.BROWSABLE\" />\n");
    }
    for s in f.schemes {
        out.push_str(&format!("        <data android:scheme=\"{}\" />\n", attr(s)));
    }
    if let Some(mime) = f.mime {
        out.push_str(&format!("        <data android:mimeType=\"{}\" />\n", attr(mime)));
    }
    out.push_str("    </intent-filter>\n");
}
