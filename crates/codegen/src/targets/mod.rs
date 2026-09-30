//! 各生成目标。类型化接口（typescript、csharp、swift、kotlin、python、dart）与原生意图框架
//! （swift-app-intents、kotlin-appfunctions、windows-app-actions）共用 [`crate::schema`] 的模型。

pub mod app_intents;
pub mod appfunctions;
pub mod csharp;
pub mod dart;
pub mod kotlin;
pub mod python;
pub mod swift;
pub mod typescript;
pub mod windows;

use std::path::PathBuf;

use app_mcp_protocol::Risk;

use crate::schema::{Model, ToolModel, Warning};
use crate::{GeneratedFile, Options, Target, ident};

pub fn generate(
    model: &Model,
    target: Target,
    options: &Options,
    warnings: &mut Vec<Warning>,
) -> Vec<GeneratedFile> {
    match target {
        Target::TypeScript => vec![typescript::generate(model)],
        Target::CSharp => vec![csharp::generate(model, &csharp_namespace(model, options))],
        Target::Swift => vec![swift::generate(model)],
        Target::Kotlin => vec![kotlin::generate(model, &kotlin_package(model, options))],
        Target::Python => vec![python::generate(model)],
        Target::Dart => vec![dart::generate(model)],
        Target::SwiftAppIntents => app_intents::generate(model, warnings),
        Target::KotlinAppFunctions => {
            appfunctions::generate(model, &kotlin_package(model, options), warnings)
        }
        Target::WindowsAppActions => {
            windows::generate(model, &csharp_namespace(model, options), warnings)
        }
    }
}

/// Kotlin 包名：`--package`，缺省为 `appmcp.generated.<appId>`。
pub fn kotlin_package(model: &Model, options: &Options) -> String {
    options
        .package
        .clone()
        .unwrap_or_else(|| format!("appmcp.generated.{}", ident::snake(&model.app_id, "app")))
}

/// C# 命名空间：`--package`，缺省为 `AppMcp.Generated.<Module>`。
pub fn csharp_namespace(model: &Model, options: &Options) -> String {
    options
        .package
        .clone()
        .unwrap_or_else(|| format!("AppMcp.Generated.{}", model.module))
}

pub fn file(path: impl Into<PathBuf>, contents: String) -> GeneratedFile {
    GeneratedFile {
        path: path.into(),
        contents,
    }
}

pub fn risk_name(risk: Risk) -> &'static str {
    match risk {
        Risk::Read => "read",
        Risk::Write => "write",
        Risk::Destructive => "destructive",
        Risk::Payment => "payment",
        Risk::OsSensitive => "os-sensitive",
    }
}

/// 需要用户确认的风险等级。
pub fn needs_confirmation(risk: Risk) -> bool {
    matches!(risk, Risk::Destructive | Risk::Payment | Risk::OsSensitive)
}

/// 工具的文档注释行：描述 + 工具名与风险。
pub fn tool_doc(tool: &ToolModel) -> Vec<String> {
    let mut lines: Vec<String> = tool
        .info
        .description
        .lines()
        .map(|l| l.trim_end().to_string())
        .collect();
    lines.push(String::new());
    let title = tool
        .info
        .title
        .as_deref()
        .map(|t| format!("「{t}」"))
        .unwrap_or_default();
    lines.push(format!(
        "工具 `{}`{title}，风险：{}",
        tool.info.name,
        risk_name(tool.info.risk)
    ));
    lines
}

/// 类型声明的文档注释行。
pub fn decl_doc(description: Option<&str>) -> Vec<String> {
    crate::code::doc_lines(description)
}
