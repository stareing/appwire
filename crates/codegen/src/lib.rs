//! app-mcp-codegen：从静态清单 `app-mcp.json` 生成
//!
//! - 各平台原生"App 能力声明"框架的代码：Apple App Intents（Swift）、Android AppFunctions（Kotlin）、
//!   Windows App Actions（Action 定义 JSON + C# 处理骨架）、鸿蒙意图框架（InsightIntent 装饰器执行器 +
//!   insight_intent.json + ArkTS 类型）；
//! - 各语言的类型化接口（参数类型 + handler 接口）：TypeScript、C#、Swift、Kotlin、Python、Dart。
//!
//! JSON Schema → 类型的映射在 [`schema`] 中一处实现，各 target 共用。

pub mod code;
pub mod ident;
pub mod ordered;
pub mod schema;
pub mod targets;

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use app_mcp_manifest::Manifest;

pub use schema::{Model, Warning};

/// 生成目标。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    SwiftAppIntents,
    KotlinAppFunctions,
    WindowsAppActions,
    HarmonyInsightIntents,
    TypeScript,
    CSharp,
    Swift,
    Kotlin,
    Python,
    Dart,
}

impl Target {
    pub const ALL: [Target; 10] = [
        Target::SwiftAppIntents,
        Target::KotlinAppFunctions,
        Target::WindowsAppActions,
        Target::HarmonyInsightIntents,
        Target::TypeScript,
        Target::CSharp,
        Target::Swift,
        Target::Kotlin,
        Target::Python,
        Target::Dart,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Target::SwiftAppIntents => "swift-app-intents",
            Target::KotlinAppFunctions => "kotlin-appfunctions",
            Target::WindowsAppActions => "windows-app-actions",
            Target::HarmonyInsightIntents => "harmony-insight-intents",
            Target::TypeScript => "typescript",
            Target::CSharp => "csharp",
            Target::Swift => "swift",
            Target::Kotlin => "kotlin",
            Target::Python => "python",
            Target::Dart => "dart",
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Target {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Target::ALL
            .into_iter()
            .find(|t| t.name() == s)
            .ok_or_else(|| {
                let names: Vec<&str> = Target::ALL.iter().map(|t| t.name()).collect();
                format!("未知的 target `{s}`，可选：{}", names.join("、"))
            })
    }
}

/// 生成选项。
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// 包名 / 命名空间（Kotlin package、C# namespace、Dart library）。缺省时由 appId 推导。
    pub package: Option<String>,
    /// 模块名（PascalCase 前缀，如 `Shop` → `ShopToolHandlers`）。缺省时由 appId 推导。
    pub module: Option<String>,
    /// 鸿蒙意图垂域（`harmony-insight-intents`），缺省 `ToolsDomain`。
    pub intent_domain: Option<String>,
    /// 鸿蒙意图绑定的 UIAbility 名（`harmony-insight-intents`），缺省 `EntryAbility`。
    pub ability: Option<String>,
}

/// 一个生成的文件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedFile {
    /// 相对于输出目录的路径。
    pub path: PathBuf,
    pub contents: String,
}

/// 生成结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    pub files: Vec<GeneratedFile>,
    pub warnings: Vec<Warning>,
}

/// 为清单生成指定 target 的代码。
///
/// 已解析的 [`Manifest`] 中对象键按名称排序（serde_json 默认行为），生成的字段顺序也按名称排序；
/// 需要保持清单中的声明顺序时用 [`generate_from_str`]。
pub fn generate(manifest: &Manifest, target: Target, options: &Options) -> Output {
    generate_ordered(manifest, None, target, options)
}

/// 解析、校验清单原文并生成代码；字段顺序与清单中的声明顺序一致。
pub fn generate_from_str(
    text: &str,
    target: Target,
    options: &Options,
) -> Result<(Output, Vec<app_mcp_manifest::Issue>), app_mcp_manifest::ManifestError> {
    let loaded = app_mcp_manifest::load_str(text)?;
    // 原文已能被 serde_json 解析，这里的二次解析不会失败；失败时退回按名称排序
    let order = ordered::parse(text).ok();
    let output = generate_ordered(&loaded.manifest, order.as_ref(), target, options);
    Ok((output, loaded.warnings))
}

fn generate_ordered(
    manifest: &Manifest,
    order: Option<&ordered::Node>,
    target: Target,
    options: &Options,
) -> Output {
    let model = schema::build(manifest, options.module.as_deref(), order);
    let mut warnings = model.warnings.clone();
    let files = targets::generate(&model, target, options, &mut warnings);
    Output { files, warnings }
}
