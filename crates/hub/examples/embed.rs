//! 嵌入式 Hub 最小示例：
//!
//! 1. 启动 Hub（WebSocket 监听随机端口）；
//! 2. 在同一进程里用 `app-mcp-native`（App 端 SDK）起一个 App，注册工具 `notes.add`；
//! 3. `export_tools(Anthropic)` 得到给模型的工具定义；
//! 4. 假装模型返回了一个 `tool_use`，交给 `dispatch` 执行，打印 `tool_result`。
//!
//! 运行：`cargo run -p app-mcp-hub --example embed`

use std::sync::Arc;
use std::time::Duration;

use app_mcp_hub::{Hub, HubConfig, HubEvent, ToolFilter, ToolFormat};
use app_mcp_native::{AppOverview, CallHandle, NativeClient, NativeConfig, Risk, ToolHandler, ToolSpec};
use serde_json::{Value, json};

/// App 侧的工具实现：在分发线程上被调用，直接完成。
struct AddNote;

impl ToolHandler for AddNote {
    fn invoke(&self, call: CallHandle) {
        let args: Value = serde_json::from_str(&call.arguments_json()).unwrap_or_default();
        let text = args["text"].as_str().unwrap_or_default();
        let data = json!({ "ok": true, "saved": text });
        let _ = call.complete(Some(&data.to_string()), vec!["notes.list".to_owned()]);
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // ---- 1. 启动 Hub ----
    let hub = Hub::start(HubConfig {
        ws_addr: Some("127.0.0.1:0".into()),
        // 演示用随机端口；不占用默认 IPC 端点（常驻 Host 可能正在使用）。
        ipc_endpoint: None,
        ..Default::default()
    })
    .await?;
    let addr = hub.ws_addr().ok_or_else(|| anyhow::anyhow!("没有 WebSocket 地址"))?;
    let mut events = hub.events();
    println!("Hub 已启动：ws://{addr}");

    // ---- 2. 同进程 App（实际产品中是另一个进程 / 网页）----
    let mut cfg = NativeConfig::new("demo-notes", "演示笔记");
    cfg.host_url = format!("ws://{addr}");
    cfg.instance_id = Some("embed-1".into());
    cfg.overview = Some(AppOverview {
        summary: "演示用笔记 App，可以添加笔记".into(),
        body: Some("添加笔记用 notes.add，参数 text 为笔记内容。".into()),
        locale: Some("zh-CN".into()),
    });
    let app = NativeClient::new(cfg, None)?;
    let mut spec = ToolSpec::new("notes.add", "添加一条笔记");
    spec.input_schema_json = Some(
        json!({
            "type": "object",
            "properties": { "text": { "type": "string", "description": "笔记内容" } },
            "required": ["text"]
        })
        .to_string(),
    );
    spec.risk = Risk::Write;
    let _tool = app.register_tool(spec, Arc::new(AddNote))?;
    app.start();

    // 等 App 连上并同步完工具
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut connected = false;
        while let Ok(ev) = events.recv().await {
            println!("事件：{}", serde_json::to_string(&ev).unwrap_or_default());
            match ev {
                HubEvent::AppConnected { .. } => connected = true,
                HubEvent::ToolsChanged if connected => break,
                _ => {}
            }
        }
    })
    .await?;

    // ---- 3. 导出工具定义（交给自有 LLM）----
    let filter = ToolFilter {
        include_builtin: false,
        ..Default::default()
    };
    let tools = hub.export_tools(ToolFormat::Anthropic, &filter);
    println!("\n导出的工具（Anthropic 格式）：\n{}", serde_json::to_string_pretty(&tools)?);

    // ---- 4. 伪 LLM 循环：模型返回 tool_use → dispatch → tool_result ----
    let tool_use = json!({
        "type": "tool_use",
        "id": "toolu_01",
        "name": tools[0]["name"],
        "input": { "text": "买牛奶" }
    });
    println!("\n模型返回：{tool_use}");
    let tool_result = hub.dispatch(ToolFormat::Anthropic, tool_use).await;
    println!(
        "\ndispatch 结果（放进下一轮 user 消息）：\n{}",
        serde_json::to_string_pretty(&tool_result)?
    );

    app.stop();
    hub.shutdown().await;
    Ok(())
}
