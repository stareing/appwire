//! `app-mcp-codegen --manifest app-mcp.json --target <target> --out <dir> [--package <name>] [--module <name>]
//! [--intent-domain <垂域>] [--ability <UIAbility>]
//! [--app-intents-extension] [--app-intents-execution-targets] [--app-intents-cancellable] [--standard-intents]`

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use app_mcp_codegen::{AppIntentsOptions, Options, Target, generate_from_str};
use clap::Parser;

/// 从 app-mcp.json 生成各平台原生意图声明与各语言类型化接口。
#[derive(Debug, Parser)]
#[command(name = "app-mcp-codegen", version)]
struct Args {
    /// 清单文件路径。
    #[arg(long)]
    manifest: PathBuf,
    /// 生成目标：swift-app-intents、kotlin-appfunctions、windows-app-actions、harmony-insight-intents、
    /// typescript、csharp、swift、kotlin、python、dart。
    #[arg(long)]
    target: Target,
    /// 输出目录（不存在时创建）。
    #[arg(long)]
    out: PathBuf,
    /// 包名 / 命名空间（Kotlin package、C# namespace、Dart library）。
    #[arg(long)]
    package: Option<String>,
    /// 模块名（生成类型的前缀，如 Shop → ShopToolHandlers），缺省由 appId 推导。
    #[arg(long)]
    module: Option<String>,
    /// 鸿蒙意图垂域（harmony-insight-intents），缺省 ToolsDomain。
    #[arg(long)]
    intent_domain: Option<String>,
    /// 鸿蒙意图绑定的 UIAbility 名（harmony-insight-intents），缺省 EntryAbility。
    #[arg(long)]
    ability: Option<String>,
    /// swift-app-intents：改为"共享 Swift 包 + App Intents 扩展"布局，App 不运行时也能由扩展执行 intent。
    #[arg(long)]
    app_intents_extension: bool,
    /// swift-app-intents：按 activation 声明 allowedExecutionTargets（iOS / macOS 27 起，以 @available 限定）。
    #[arg(long)]
    app_intents_execution_targets: bool,
    /// swift-app-intents：intent 遵循 CancellableIntent（iOS / macOS 26.4 起，以 #available 限定），把取消原因转给 App。
    #[arg(long)]
    app_intents_cancellable: bool,
    /// swift-app-intents / kotlin-appfunctions / harmony-insight-intents：为声明了 implements 的工具额外输出
    /// 系统意图版本（spec/intents.md 第 3 节）。
    #[arg(long)]
    standard_intents: bool,
}

fn run(args: Args) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(&args.manifest)
        .with_context(|| format!("读取清单 {} 失败", args.manifest.display()))?;
    let app_intents = AppIntentsOptions {
        extension: args.app_intents_extension,
        execution_targets: args.app_intents_execution_targets,
        cancellable: args.app_intents_cancellable,
    };
    if app_intents != AppIntentsOptions::default() && args.target != Target::SwiftAppIntents {
        anyhow::bail!("--app-intents-* 选项只用于 --target swift-app-intents（当前为 {}）", args.target);
    }
    if args.standard_intents && !args.target.supports_standard_intents() {
        anyhow::bail!(
            "--standard-intents 只用于 swift-app-intents、kotlin-appfunctions、harmony-insight-intents（当前为 {}）",
            args.target
        );
    }
    let options = Options {
        package: args.package,
        module: args.module,
        intent_domain: args.intent_domain,
        ability: args.ability,
        app_intents,
        standard_intents: args.standard_intents,
    };
    let (output, manifest_warnings) = generate_from_str(&text, args.target, &options)
        .with_context(|| format!("清单 {} 无效", args.manifest.display()))?;
    for w in &manifest_warnings {
        eprintln!("清单警告：{w}");
    }
    for w in &output.warnings {
        eprintln!("警告：{w}");
    }
    for file in &output.files {
        let path = args.out.join(&file.path);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("创建目录 {} 失败", dir.display()))?;
        }
        std::fs::write(&path, &file.contents)
            .with_context(|| format!("写入 {} 失败", path.display()))?;
        eprintln!("已生成 {}", path.display());
    }
    Ok(())
}

fn main() -> ExitCode {
    match run(Args::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("错误：{e:#}");
            ExitCode::FAILURE
        }
    }
}
