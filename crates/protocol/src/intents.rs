//! 标准意图（spec/intents.md，第 16 项 N4）：词表的机器可读形式、`"<动词>@<主版本>"` 的解析、工具 `implements` 的格式校验
//! 与兼容性检查。清单校验、核心注册与 Hub 的 `apps.intents` 共用这里的定义（一处定义）。
//!
//! 只做纯数据判断，不做 I/O；本库不校验工具是否真的实现了动词的语义（归 App）。

use std::fmt;

use serde_json::Value;

/// 工具 `implements` 的最多项数（spec/intents.md 第 1 节）。
pub const MAX_IMPLEMENTS: usize = 4;

/// 动词名（`<域>.<动作>`）的最大长度。
///
/// @why 与工具名上限一致（`[a-zA-Z0-9_.-]{1,64}`）；spec/intents.md 只规定了形式，没有给长度，取工具名上限限制列表与清单体积。
pub const MAX_VERB_LEN: usize = 64;

/// 必填参数的 JSON 类型（兼容性检查比较 `inputSchema.properties.<名>.type`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamKind {
    String,
    /// 元素为字符串的数组（`items.type` 若声明须为 `string`）。
    StringArray,
    Boolean,
    Object,
}

impl ParamKind {
    /// 对应的 JSON Schema `type`。
    pub fn json_type(self) -> &'static str {
        match self {
            ParamKind::String => "string",
            ParamKind::StringArray => "array",
            ParamKind::Boolean => "boolean",
            ParamKind::Object => "object",
        }
    }

    /// 数组元素的 JSON Schema `type`；不是数组时为 `None`。
    pub fn item_type(self) -> Option<&'static str> {
        matches!(self, ParamKind::StringArray).then_some("string")
    }
}

/// 词表中的一个必填参数。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IntentParam {
    pub name: &'static str,
    pub kind: ParamKind,
}

/// 词表中一个动词的一个主版本。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IntentDef {
    /// `<域>.<动作>`，如 `message.send`。
    pub verb: &'static str,
    pub version: u32,
    /// 面向模型的一句说明（`apps.intents` 的 `description`）。
    pub description: &'static str,
    /// 必填参数（兼容性检查只看这些；可选参数不检查）。
    pub required: &'static [IntentParam],
}

impl IntentDef {
    /// `"<动词>@<主版本>"`。
    pub fn id(&self) -> String {
        format!("{}@{}", self.verb, self.version)
    }
}

const fn p(name: &'static str, kind: ParamKind) -> IntentParam {
    IntentParam { name, kind }
}

/// 词表（spec/intents.md 第 2 节，版本 1）。新增动词或版本时同时改 spec；不兼容的变更只能发新主版本，旧版本保留。
pub const VOCABULARY: [IntentDef; 6] = [
    IntentDef {
        verb: "message.send",
        version: 1,
        description: "发送消息：to 为收件人（联系人名、号码、地址或 App 内 ID，至少一个），text 为正文；可选 subject、attachments。",
        required: &[p("to", ParamKind::StringArray), p("text", ParamKind::String)],
    },
    IntentDef {
        verb: "calendar.create",
        version: 1,
        description: "新建日程：title 为标题，start 为开始时间（RFC 3339）；可选 end、allDay、location、attendees、notes。",
        required: &[p("title", ParamKind::String), p("start", ParamKind::String)],
    },
    IntentDef {
        verb: "media.play",
        version: 1,
        description: "播放媒体：query 为要播放什么（有 uri 时可为空字符串）；可选 uri、kind（song / album / artist / playlist / podcast / video）。",
        required: &[p("query", ParamKind::String)],
    },
    IntentDef {
        verb: "file.share",
        version: 1,
        description: "分享文件：files 为文件 URI 或 Hub 句柄（至少一个）；可选 mimeType、text、to。",
        required: &[p("files", ParamKind::StringArray)],
    },
    IntentDef {
        verb: "link.open",
        version: 1,
        description: "打开链接：url 为要打开的地址。",
        required: &[p("url", ParamKind::String)],
    },
    IntentDef {
        verb: "navigation.start",
        version: 1,
        description: "开始导航：destination 为目的地对象（name / address / lat+lng 至少一组）；可选 mode（drive / walk / transit / bike）。",
        required: &[p("destination", ParamKind::Object)],
    },
];

/// 词表中的动词版本；不在词表中时为 `None`。
pub fn lookup(verb: &str, version: u32) -> Option<&'static IntentDef> {
    VOCABULARY.iter().find(|d| d.verb == verb && d.version == version)
}

/// 词表中是否有这个动词（任意版本）。
pub fn is_known_verb(verb: &str) -> bool {
    VOCABULARY.iter().any(|d| d.verb == verb)
}

/// 意图格式错误（清单为错误、核心注册失败；spec/intents.md 第 1 节）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IntentError {
    /// 不是 `<动词>@<主版本>`（或动词名不是 `<域>.<动作>`）。
    Format(String),
    /// `implements` 中出现重复项。
    Duplicate(String),
    /// `implements` 超过 [`MAX_IMPLEMENTS`] 项。
    TooMany(usize),
}

impl fmt::Display for IntentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IntentError::Format(s) => write!(
                f,
                "标准意图 \"{s}\" 格式不合法，应为 \"<域>.<动作>@<主版本>\"（如 \"message.send@1\"，主版本为正整数，spec/intents.md）"
            ),
            IntentError::Duplicate(s) => write!(f, "implements 中 \"{s}\" 重复"),
            IntentError::TooMany(n) => write!(f, "implements 最多 {MAX_IMPLEMENTS} 项（实际 {n} 项）"),
        }
    }
}

impl std::error::Error for IntentError {}

/// 一个意图引用 `"<动词>@<主版本>"`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IntentRef<'a> {
    pub verb: &'a str,
    pub version: u32,
}

impl<'a> IntentRef<'a> {
    /// 解析 `"<动词>@<主版本>"`。
    ///
    /// @error 动词名不合法（[`is_valid_verb`]）、缺少 `@`、主版本不是无前导零的正整数 → [`IntentError::Format`]。
    pub fn parse(text: &'a str) -> Result<Self, IntentError> {
        match parse_verb_query(text) {
            Ok((verb, Some(version))) => Ok(Self { verb, version }),
            _ => Err(IntentError::Format(text.to_owned())),
        }
    }

    /// 词表中的定义；未知动词或版本为 `None`。
    pub fn lookup(&self) -> Option<&'static IntentDef> {
        lookup(self.verb, self.version)
    }
}

impl fmt::Display for IntentRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.verb, self.version)
    }
}

/// 动词名 `<域>.<动作>`：恰好两段，以 `.` 分隔；每段以 ASCII 字母开头，其余为 ASCII 字母、数字、`_`、`-`；总长不超过
/// [`MAX_VERB_LEN`]。
pub fn is_valid_verb(verb: &str) -> bool {
    let segment = |s: &str| {
        let mut bytes = s.bytes();
        bytes.next().is_some_and(|b| b.is_ascii_alphabetic()) && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    };
    verb.len() <= MAX_VERB_LEN && verb.split_once('.').is_some_and(|(domain, action)| segment(domain) && segment(action))
}

/// 主版本：无前导零的正整数（`u32` 范围内）。
fn parse_version(text: &str) -> Option<u32> {
    let valid = !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) && !text.starts_with('0');
    valid.then(|| text.parse().ok()).flatten()
}

/// 解析可带可不带版本的动词：`message.send`（任意版本）或 `message.send@1`（`apps.intents` 的 `intent` 参数、机主默认表的键）。
///
/// @error 动词名或主版本不合法 → [`IntentError::Format`]。
pub fn parse_verb_query(text: &str) -> Result<(&str, Option<u32>), IntentError> {
    let format = || IntentError::Format(text.to_owned());
    let (verb, version) = match text.split_once('@') {
        None => (text, None),
        Some((verb, v)) => (verb, Some(parse_version(v).ok_or_else(format)?)),
    };
    if !is_valid_verb(verb) {
        return Err(format());
    }
    Ok((verb, version))
}

/// 工具 `implements` 的全部格式问题（格式 / 重复 / 上限），按出现顺序；`index` 为出错项的下标，上限问题为 `None`。
pub fn implements_errors(list: &[String]) -> Vec<(Option<usize>, IntentError)> {
    let mut errors = Vec::new();
    if list.len() > MAX_IMPLEMENTS {
        errors.push((None, IntentError::TooMany(list.len())));
    }
    for (i, item) in list.iter().enumerate() {
        if let Err(e) = IntentRef::parse(item) {
            errors.push((Some(i), e));
        } else if list[..i].contains(item) {
            errors.push((Some(i), IntentError::Duplicate(item.clone())));
        }
    }
    errors
}

/// 工具 `implements` 的格式校验（第一个问题）。
///
/// @error 见 [`implements_errors`]。
pub fn validate_implements(list: &[String]) -> Result<(), IntentError> {
    implements_errors(list).into_iter().next().map_or(Ok(()), |(_, e)| Err(e))
}

/// 一个工具相对一个意图的状态。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Compatibility {
    /// 词表中的动词版本，必填参数齐全、类型一致。
    Compatible,
    /// 词表中的动词版本，但不满足兼容性要求（原因面向模型 / 开发者）。
    Incompatible(String),
    /// 动词或版本不在词表中：无从检查（Hub 照常列出，`known: false`）。
    Unknown,
}

/// 检查工具 `inputSchema` 是否满足意图的必填参数（spec/intents.md 第 1 节「兼容性检查」）。
pub fn compatibility(intent: IntentRef<'_>, input_schema: &Value) -> Compatibility {
    match intent.lookup() {
        None => Compatibility::Unknown,
        Some(def) => match check_required(def, input_schema) {
            Ok(()) => Compatibility::Compatible,
            Err(reason) => Compatibility::Incompatible(reason),
        },
    }
}

/// 必填参数须出现在 `inputSchema.properties` 中，且类型（若声明）一致；不满足时返回全部原因（以「；」连接）。
///
/// @error 原因文本，如 `缺少必填参数 text；参数 to 的类型应为 array（声明为 string）`。
pub fn check_required(def: &IntentDef, input_schema: &Value) -> Result<(), String> {
    let props = input_schema.get("properties").and_then(Value::as_object);
    let reasons: Vec<String> = def
        .required
        .iter()
        .filter_map(|param| match props.and_then(|p| p.get(param.name)) {
            None => Some(format!("缺少必填参数 {}", param.name)),
            Some(schema) => param_mismatch(param, schema),
        })
        .collect();
    if reasons.is_empty() { Ok(()) } else { Err(reasons.join("；")) }
}

/// 参数 schema 与词表类型不一致的原因；未声明类型视为一致。
fn param_mismatch(param: &IntentParam, schema: &Value) -> Option<String> {
    let expected = param.kind.json_type();
    if let Some(declared) = schema.get("type").filter(|t| !type_allows(t, expected)) {
        return Some(format!("参数 {} 的类型应为 {expected}（声明为 {declared}）", param.name));
    }
    let item = param.kind.item_type()?;
    let declared = schema.get("items")?.get("type").filter(|t| !type_allows(t, item))?;
    Some(format!("参数 {} 的元素类型应为 {item}（声明为 {declared}）", param.name))
}

/// JSON Schema `type`（字符串或字符串数组）是否允许 `expected`；`type` 不是这两种形式时视为不一致。
fn type_allows(declared: &Value, expected: &str) -> bool {
    match declared {
        Value::String(s) => s == expected,
        Value::Array(items) => items.iter().any(|t| t.as_str() == Some(expected)),
        _ => false,
    }
}

#[cfg(test)]
mod tests;
