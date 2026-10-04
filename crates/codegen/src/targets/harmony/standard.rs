//! 鸿蒙意图框架标准意图（`--standard-intents`，spec/intents.md 第 3 节）：为声明了 `implements` 的工具额外输出
//! `@InsightIntentEntry({ schema, intentVersion })` 执行器，执行时转换参数后调用同一个 `<Module>ToolHandlers` 方法。
//!
//! 输出（路径相对于模块的 `src/main/`，追加到 `insight_intent.json` 的 `insightIntentsSrcEntry`）：
//! - `ets/insightintents/standard/<Module><标准意图>StandardIntent.ets`：每个标准意图一个执行器；
//! - `ets/appmcp/<Module>StandardIntents.ets`（有媒体意图时）：实体解析接口 `<Module>MediaEntityResolver` 与注入点。
//!
//! 映射（只收 SDK 中有 schema 文件的标准意图）：
//! - `media.play@1` → `PlayVideo` / `PlayAudio` / `PlayMusicList`：按工具 `kind` 枚举中的 `video` / `podcast` / `playlist` 选择；
//!   系统只传 App 侧实体 ID（`entityId`），由 App 实现的解析器转换为工具参数；
//! - `navigation.start@1` → `StartNavigate`：`dstLocation` 转为 `destination`，`trafficType` 转为 `mode`；
//! - 其余动词在鸿蒙标准意图中没有对应（由 [`crate::standard_intents::bindings`] 警告），只生成自定义意图。
//!
//! 与选项的关系：`--ability` 作用于标准意图执行器；`--intent-domain` 不作用（垂域由标准意图 schema 固定）。
//!
//! 依据（OpenHarmony SDK 6.0.0.47 / API 20）：
//! - ets-loader `insight_intents/schema/<名称>_<版本>.json`（标准意图的名称、版本、垂域与参数）；
//! - ets-loader `lib/userIntents_parser/parseUserIntents.js` 的 `collectSchemaInfo`（按 `schema` 与 `intentVersion` 读取上述文件，
//!   覆盖 `intentName` / `domain` / `llmDescription` / `keywords` / `parameters` / `result`；文件不存在时不覆盖也不报错）
//!   与 `schemaValidateSync`（执行器属性须与 schema 类型一致，`object` 类型的属性须为 `@InsightIntentEntity` 类）；
//! - docs `application-models/insight-intent-decorator-development.md`「通过意图装饰器开发标准意图」、
//!   `insight-intent-access-specifications.md`（PlayVideo / PlayMusicList / PlayAudio / StartNavigate）。

use super::*;
use crate::standard_intents::{self, Binding};
use app_mcp_protocol::intents::IntentDef;

mod emit;

/// 鸿蒙标准意图的执行器属性。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prop {
    pub name: &'static str,
    /// ArkTS 类型；[`ENTITY_TYPE`] 表示生成的位置实体类。
    pub ty: &'static str,
    pub required: bool,
    pub doc: &'static str,
}

/// 属性类型占位：替换为生成的 `@InsightIntentEntity` 位置类。
pub const ENTITY_TYPE: &str = "<entity>";

const fn prop(name: &'static str, ty: &'static str, required: bool, doc: &'static str) -> Prop {
    Prop {
        name,
        ty,
        required,
        doc,
    }
}

/// 一个鸿蒙标准意图（SDK `insight_intents/schema/<name>_<version>.json`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StandardIntent {
    /// `schema` 与 `intentName`。
    pub name: &'static str,
    /// `intentVersion`，须与 SDK schema 文件名中的版本一致。
    pub version: &'static str,
    pub domain: &'static str,
    /// 执行器声明的属性（schema 中 `object` 类型且无法对应到词表的属性不声明，系统不赋值）。
    pub props: &'static [Prop],
    /// `media.play` 按工具 `kind` 枚举选择时对应的取值。
    pub media_kind: Option<&'static str>,
}

const ENTITY_ID_DOC: &str = "意图实体 ID（App 提供给系统的内容 ID，长度不超过 64 字符）。";

pub const PLAY_VIDEO: StandardIntent = StandardIntent {
    name: "PlayVideo",
    version: "1.0.2",
    domain: "MediaDomain",
    props: &[
        prop("entityId", "string", true, ENTITY_ID_DOC),
        prop("episodeId", "string", false, "集数标识。"),
        prop("episodeNumber", "number", false, "目标集数。"),
    ],
    media_kind: Some("video"),
};

pub const PLAY_AUDIO: StandardIntent = StandardIntent {
    name: "PlayAudio",
    version: "1.0.1",
    domain: "MediaDomain",
    props: &[
        prop("entityId", "string", true, ENTITY_ID_DOC),
        prop("soundId", "string", false, "有声节目 ID。"),
    ],
    media_kind: Some("podcast"),
};

pub const PLAY_MUSIC_LIST: StandardIntent = StandardIntent {
    name: "PlayMusicList",
    version: "1.0.2",
    domain: "MediaDomain",
    props: &[
        prop("entityId", "string", false, ENTITY_ID_DOC),
        prop(
            "entityGroupId",
            "string",
            false,
            "歌单的 UI 形式（App 自定义）。",
        ),
        prop(
            "sceneType",
            "string",
            false,
            "场景类型，如 MORNING_SCENE、DRIVE_SCENE。",
        ),
        prop("city", "string", false, "城市名。"),
    ],
    media_kind: Some("playlist"),
};

pub const START_NAVIGATE: StandardIntent = StandardIntent {
    name: "StartNavigate",
    version: "1.0.1",
    domain: "NavigationDomain",
    props: &[
        prop("entityId", "string", false, ENTITY_ID_DOC),
        prop(
            "dstLocation",
            ENTITY_TYPE,
            false,
            "目的地（坐标系缺省为 GCJ-02）。",
        ),
        prop(
            "trafficType",
            "string",
            false,
            "交通方式：Drive / Walk / Cycle / Bus / Subway。",
        ),
    ],
    media_kind: None,
};

/// `media.play` 的候选标准意图（按此顺序生成）。
pub const MEDIA_INTENTS: [StandardIntent; 3] = [PLAY_VIDEO, PLAY_AUDIO, PLAY_MUSIC_LIST];

/// `StartNavigate.trafficType` → 词表 `mode`。
pub const TRAFFIC_MODES: [(&str, &str); 5] = [
    ("Drive", "drive"),
    ("Walk", "walk"),
    ("Cycle", "bike"),
    ("Bus", "transit"),
    ("Subway", "transit"),
];

/// 词表 `destination` 的子字段 ← `dstLocation` 的属性；`numeric` 为真时由字符串解析为数值（只在 WGS-84 时传递）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DestinationSource {
    pub target: &'static str,
    pub source: &'static str,
    pub numeric: bool,
}

pub const DESTINATION_SOURCES: [DestinationSource; 4] = [
    DestinationSource {
        target: "name",
        source: "locationName",
        numeric: false,
    },
    DestinationSource {
        target: "address",
        source: "address",
        numeric: false,
    },
    DestinationSource {
        target: "lat",
        source: "latitude",
        numeric: true,
    },
    DestinationSource {
        target: "lng",
        source: "longitude",
        numeric: true,
    },
];

/// 词表动词在鸿蒙上的标准意图族（[`standard_intents::bindings`] 的映射表）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Media,
    Navigation,
}

pub fn family(def: &IntentDef) -> Option<Family> {
    match (def.verb, def.version) {
        ("media.play", 1) => Some(Family::Media),
        ("navigation.start", 1) => Some(Family::Navigation),
        _ => None,
    }
}

/// `destination` 的一个可填子字段。
#[derive(Clone, Debug)]
pub struct DestinationField<'m> {
    pub field: &'m Field,
    pub source: DestinationSource,
}

/// `navigation.start` 的参数转换方案。
#[derive(Clone, Debug)]
pub struct NavigationPlan<'m> {
    pub destination: &'m Field,
    pub destination_decl: &'m ObjectDecl,
    pub fields: Vec<DestinationField<'m>>,
    /// 工具的 `mode` 字段与可用的 `trafficType` → `mode` 映射。
    pub mode: Option<(&'m Field, Vec<(&'static str, &'static str)>)>,
}

/// 一个要生成的标准意图执行器。
#[derive(Clone, Debug)]
pub struct Plan<'m> {
    pub tool: &'m ToolModel,
    /// 工具声明的词表动词版本。
    pub verb: &'static IntentDef,
    pub intent: StandardIntent,
    /// 导航意图的参数转换；媒体意图为 `None`（经 App 的实体解析器）。
    pub navigation: Option<NavigationPlan<'m>>,
}

/// 生成标准意图执行器与（有媒体意图时）解析器文件；返回文件与 `insightIntentsSrcEntry` 条目。
pub fn generate(
    model: &Model,
    ability: &str,
    warnings: &mut Vec<Warning>,
) -> (Vec<GeneratedFile>, Vec<String>) {
    let plans = plans(model, warnings);
    let mut files = Vec::new();
    let mut entries = Vec::new();
    if plans.iter().any(|p| p.navigation.is_none()) {
        files.push(file(
            format!("ets/appmcp/{}StandardIntents.ets", model.module),
            emit::media_file(model, &plans),
        ));
    }
    for plan in &plans {
        let name = executor_class(model, &plan.intent);
        files.push(file(
            format!("ets/insightintents/standard/{name}.ets"),
            emit::executor_file(model, plan, ability),
        ));
        entries.push(format!("./ets/insightintents/standard/{name}.ets"));
    }
    (files, entries)
}

/// 执行器类名 / 文件名：`<Module><标准意图>StandardIntent`。
pub fn executor_class(model: &Model, intent: &StandardIntent) -> String {
    format!("{}{}StandardIntent", model.module, intent.name)
}

/// 按工具顺序收集要生成的标准意图；不能生成的给出警告。
pub fn plans<'m>(model: &'m Model, warnings: &mut Vec<Warning>) -> Vec<Plan<'m>> {
    let resolve = |b: Binding<'m, Family>, w: &mut Vec<Warning>| {
        let found: Vec<Plan<'m>> = match b.system {
            Family::Media => media_plans(model, &b, w),
            Family::Navigation => navigation_plan(model, &b, w).into_iter().collect(),
        };
        (!found.is_empty()).then_some(found)
    };
    standard_intents::bindings(model, "HarmonyOS", family, resolve, warnings).into_iter().flatten().collect()
}

fn skip(warnings: &mut Vec<Warning>, tool: &ToolModel, message: String) {
    warnings.push(Warning {
        tool: tool.info.name.clone(),
        path: String::new(),
        message: format!("harmony-insight-intents 标准意图：{message}，只生成自定义意图"),
    });
}

fn find_field<'m>(o: &'m ObjectDecl, name: &str) -> Option<&'m Field> {
    o.fields.iter().find(|f| f.json_name == name)
}

/// 字段的字符串枚举取值；不是枚举时为 `None`。
fn enum_values<'m>(model: &'m Model, f: &Field) -> Option<&'m [String]> {
    match f.ty {
        Ty::Enum(id) => model.enum_decl(id).map(|e| e.values.as_slice()),
        _ => None,
    }
}

fn media_plans<'m>(
    model: &'m Model,
    b: &Binding<'m, Family>,
    warnings: &mut Vec<Warning>,
) -> Vec<Plan<'m>> {
    let tool = b.tool;
    let kinds = find_field(model.params(tool), "kind").and_then(|f| enum_values(model, f));
    let Some(kinds) = kinds else {
        skip(
            warnings,
            tool,
            format!(
                "`{}` 的鸿蒙标准意图（PlayVideo / PlayAudio / PlayMusicList）按 kind 枚举选择，本工具未声明 kind 枚举",
                b.intent.id()
            ),
        );
        return Vec::new();
    };
    let chosen: Vec<Plan<'m>> = MEDIA_INTENTS
        .iter()
        .filter(|i| i.media_kind.is_some_and(|k| kinds.iter().any(|v| v == k)))
        .map(|&intent| Plan {
            tool,
            verb: b.intent,
            intent,
            navigation: None,
        })
        .collect();
    if chosen.is_empty() {
        skip(
            warnings,
            tool,
            "kind 枚举不含 video / podcast / playlist，鸿蒙没有对应的媒体标准意图".to_string(),
        );
    }
    chosen
}

fn navigation_plan<'m>(
    model: &'m Model,
    b: &Binding<'m, Family>,
    warnings: &mut Vec<Warning>,
) -> Option<Plan<'m>> {
    let tool = b.tool;
    match navigation_conversion(model, model.params(tool)) {
        Ok(plan) => Some(Plan {
            tool,
            verb: b.intent,
            intent: START_NAVIGATE,
            navigation: Some(plan),
        }),
        Err(reason) => {
            skip(
                warnings,
                tool,
                format!("无法由 StartNavigate 的参数构造（{reason}）"),
            );
            None
        }
    }
}

/// `destination` 子字段的类型能否由来源填入。
fn destination_type_fits(source: DestinationSource, ty: &Ty) -> bool {
    if source.numeric {
        matches!(ty, Ty::Number)
    } else {
        matches!(ty, Ty::String)
    }
}

/// @error 返回不能转换的原因（工具有其他必填参数、`destination` 不是对象、子字段无法填入）。
pub fn navigation_conversion<'m>(
    model: &'m Model,
    params: &'m ObjectDecl,
) -> Result<NavigationPlan<'m>, String> {
    if let Some(f) = params
        .fields
        .iter()
        .find(|f| f.required && f.json_name != "destination")
    {
        return Err(format!("必填参数 `{}` 没有来源", f.json_name));
    }
    let destination = find_field(params, "destination").ok_or("缺少 destination")?;
    let decl = match destination.ty {
        Ty::Object(id) => model.object(id),
        _ => None,
    }
    .ok_or("destination 不是声明了属性的对象")?;
    let mut fields = Vec::new();
    for f in &decl.fields {
        let source = DESTINATION_SOURCES.iter().find(|s| s.target == f.json_name);
        match source {
            Some(&s) if destination_type_fits(s, &f.ty) => fields.push(DestinationField {
                field: f,
                source: s,
            }),
            _ if f.required => {
                return Err(format!(
                    "destination.{} 必填但无法由 dstLocation 填入",
                    f.json_name
                ));
            }
            _ => {}
        }
    }
    let has = |t: &str| fields.iter().any(|f| f.source.target == t);
    if !(has("name") || has("address") || (has("lat") && has("lng"))) {
        return Err("destination 没有可填入的 name / address / lat + lng".to_string());
    }
    let mode = find_field(params, "mode").and_then(|f| {
        let table: Vec<(&'static str, &'static str)> = match (&f.ty, enum_values(model, f)) {
            (_, Some(values)) => TRAFFIC_MODES
                .iter()
                .copied()
                .filter(|(_, m)| values.iter().any(|v| v == m))
                .collect(),
            (Ty::String, None) => TRAFFIC_MODES.to_vec(),
            _ => Vec::new(),
        };
        (!table.is_empty()).then_some((f, table))
    });
    Ok(NavigationPlan {
        destination,
        destination_decl: decl,
        fields,
        mode,
    })
}

#[cfg(test)]
mod tests;
