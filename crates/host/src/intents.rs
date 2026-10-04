//! 标准意图的机主默认表 `<home>/intents.json`（spec/intents.md 第 4 节，第 16 项 N4）：`{"defaults": {"message.send": "mail.compose.send"}}`。
//!
//! - 启动（`serve` / `stdio`）时加载；文件不存在 = 空表。文件不合法时拒绝启动（与 `policy.json` 相同）。
//! - `app-mcp-host intents reload`：把文件交给运行中的 Host（`POST /intents`），Host 校验不合法时保留之前的默认表，
//!   错误记入 `/status` 的 `intents.lastError`（`doctor` 报出）。
//! - `set` / `unset`：编辑文件（临时文件 + rename），Host 在运行时随即重载。
//! - 默认表只是提示：`apps.intents` 把默认工具排在最前，Hub 不据此路由。

use std::io;
use std::path::Path;
use std::process::ExitCode;

use anyhow::Context;
use app_mcp_hub::IntentsConfig;

use crate::config::AppHome;

/// 解析并校验默认表文件；不存在时为空表。
///
/// @error 中文说明（读取失败、JSON 不合法、键 / 值不合法），含文件路径。
pub fn validate_file(path: &Path) -> Result<IntentsConfig, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => IntentsConfig::from_json(&text).map_err(|e| format!("{}：{e}", path.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(IntentsConfig::default()),
        Err(e) => Err(format!("读取 {} 失败：{e}", path.display())),
    }
}

/// 启动时加载默认表（`serve` / `stdio`）。
pub fn load(home: &AppHome) -> anyhow::Result<IntentsConfig> {
    validate_file(&home.intents_file())
        .map_err(anyhow::Error::msg)
        .context("意图默认表文件无效，Host 不启动（运行 app-mcp-host intents validate 查看，修正或删除该文件后重试）")
}

/// 写入默认表文件（临时文件 + rename，不留下半写的文件）。
fn write(path: &Path, config: &IntentsConfig) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("创建 {} 失败", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(config)? + "\n";
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).with_context(|| format!("写入 {} 失败", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("替换 {} 失败", path.display()))
}

/// 设置一个意图的默认工具（覆盖已有的同键设置；先校验整个表）。返回之前的值。
///
/// @error 文件不合法、键或值不合法、写入失败。
pub fn set_default(path: &Path, intent: &str, tool: &str) -> anyhow::Result<Option<String>> {
    let mut config = validate_file(path).map_err(anyhow::Error::msg)?;
    let previous = config.defaults.insert(intent.to_owned(), tool.to_owned());
    app_mcp_hub::intents::defaults::validate_defaults(&config.defaults).map_err(anyhow::Error::msg)?;
    write(path, &config)?;
    Ok(previous)
}

/// 删除一个意图的默认设置；返回是否找到。
pub fn unset_default(path: &Path, intent: &str) -> anyhow::Result<bool> {
    let mut config = validate_file(path).map_err(anyhow::Error::msg)?;
    if config.defaults.remove(intent).is_none() {
        return Ok(false);
    }
    write(path, &config)?;
    Ok(true)
}

/// 默认表的多行描述（`intents show`）。
pub fn describe(config: &IntentsConfig) -> String {
    if config.defaults.is_empty() {
        return "未设置默认 App".to_owned();
    }
    config.defaults.iter().map(|(intent, tool)| format!("{intent} → {tool}")).collect::<Vec<_>>().join("\n")
}

/// 把默认表文件交给运行中的 Host。`Ok(None)` = Host 未运行。
async fn push(home: &AppHome) -> anyhow::Result<Option<usize>> {
    let Some((reg, token)) = crate::running_host(home).await else {
        return Ok(None);
    };
    let text = match std::fs::read_to_string(home.intents_file()) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => "{}".to_owned(),
        Err(e) => return Err(e).with_context(|| format!("读取 {} 失败", home.intents_file().display())),
    };
    let listen = reg.listen.as_deref().map(crate::probe_addr);
    let reply = crate::probe::post_intents(reg.ipc_endpoint.as_deref(), listen.as_deref(), token.as_deref(), &text)
        .await
        .map_err(anyhow::Error::msg)?;
    match (reply.ok, reply.error) {
        (true, _) => Ok(Some(reply.defaults.unwrap_or_default())),
        (false, e) => anyhow::bail!("Host 拒绝了新默认表，继续使用之前的默认表：{}", e.unwrap_or_else(|| "未给出原因".to_owned())),
    }
}

/// 编辑文件后：Host 在运行就重载，否则说明下次启动生效。
async fn push_after_edit(home: &AppHome) -> anyhow::Result<ExitCode> {
    match push(home).await? {
        Some(n) => println!("运行中的 Host 已重载意图默认表（{n} 条）"),
        None => println!("Host 未运行，默认表在下次启动时生效"),
    }
    Ok(ExitCode::SUCCESS)
}

/// `app-mcp-host intents …`。
pub async fn cmd(command: crate::cli::IntentsCommand) -> anyhow::Result<ExitCode> {
    use crate::cli::IntentsCommand as C;
    match command {
        C::Show { home, json } => {
            let home = AppHome::resolve(home.home.as_deref())?;
            let file = validate_file(&home.intents_file());
            if json {
                let v = serde_json::json!({
                    "file": home.intents_file(),
                    "fileDefaults": file.as_ref().ok().map(|c| &c.defaults),
                    "fileError": file.as_ref().err(),
                });
                println!("{}", serde_json::to_string_pretty(&v)?);
                return Ok(ExitCode::SUCCESS);
            }
            match &file {
                Ok(c) => {
                    println!("{}：", home.intents_file().display());
                    for line in describe(c).lines() {
                        println!("  {line}");
                    }
                }
                Err(e) => println!("默认表文件无效：{e}"),
            }
            Ok(ExitCode::SUCCESS)
        }
        C::Validate { home, file } => {
            let home = AppHome::resolve(home.home.as_deref())?;
            let path = file.unwrap_or_else(|| home.intents_file());
            match validate_file(&path) {
                Ok(c) => {
                    println!("{} 合法：{} 条默认设置", path.display(), c.defaults.len());
                    Ok(ExitCode::SUCCESS)
                }
                Err(e) => {
                    eprintln!("{e}");
                    Ok(ExitCode::FAILURE)
                }
            }
        }
        C::Reload(home) => {
            let home = AppHome::resolve(home.home.as_deref())?;
            match push(&home).await? {
                Some(n) => {
                    println!("已重载 {}：{n} 条默认设置生效", home.intents_file().display());
                    Ok(ExitCode::SUCCESS)
                }
                None => {
                    println!("Host 未运行；默认表在下次启动时加载");
                    Ok(ExitCode::from(crate::EXIT_NOT_RUNNING))
                }
            }
        }
        C::Set { home, intent, tool } => {
            let home = AppHome::resolve(home.home.as_deref())?;
            match set_default(&home.intents_file(), &intent, &tool)? {
                Some(old) if old != tool => println!("已把 {intent} 的默认 App 从 {old} 改为 {tool}"),
                _ => println!("已设置 {intent} → {tool}"),
            }
            push_after_edit(&home).await
        }
        C::Unset { home, intent } => {
            let home = AppHome::resolve(home.home.as_deref())?;
            if !unset_default(&home.intents_file(), &intent)? {
                eprintln!("{intent} 没有默认设置");
                return Ok(ExitCode::FAILURE);
            }
            println!("已删除 {intent} 的默认设置");
            push_after_edit(&home).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(tag: &str) -> std::path::PathBuf {
        let n: u64 = rand::random();
        std::env::temp_dir().join(format!("app-mcp-intents-{tag}-{}-{n:x}", std::process::id())).join("intents.json")
    }

    #[test]
    fn missing_file_is_empty_and_invalid_file_is_error() {
        let path = temp_file("load");
        assert_eq!(validate_file(&path), Ok(IntentsConfig::default()));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        for bad in ["[", r#"{"defaults": {"message": "a.b"}}"#, r#"{"defaults": {"message.send": "nope"}}"#, r#"{"other": 1}"#] {
            std::fs::write(&path, bad).unwrap();
            let e = validate_file(&path).unwrap_err();
            assert!(e.contains("intents.json"), "{bad}: {e}");
        }
        std::fs::write(&path, r#"{"defaults": {"message.send@1": "mail.compose.send"}}"#).unwrap();
        assert_eq!(validate_file(&path).unwrap().defaults.len(), 1);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn set_and_unset_edit_the_file() {
        let path = temp_file("edit");
        assert_eq!(set_default(&path, "message.send", "mail.compose.send").unwrap(), None);
        assert_eq!(set_default(&path, "message.send", "chat.post").unwrap().as_deref(), Some("mail.compose.send"));
        assert!(set_default(&path, "message", "chat.post").is_err(), "键不合法");
        assert!(set_default(&path, "media.play", "chat").is_err(), "值不合法");
        let c = validate_file(&path).unwrap();
        assert_eq!(c.defaults.into_iter().collect::<Vec<_>>(), [("message.send".to_owned(), "chat.post".to_owned())], "失败的设置不写入");
        assert!(describe(&validate_file(&path).unwrap()).contains("message.send → chat.post"));
        assert!(unset_default(&path, "message.send").unwrap());
        assert!(!unset_default(&path, "message.send").unwrap());
        assert_eq!(describe(&validate_file(&path).unwrap()), "未设置默认 App");
        assert!(!path.with_extension("json.tmp").exists());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
