//! 测试用的最小上游 MCP 服务器（stdio）：
//! - 工具 `echo({text})`：原样返回文本；
//! - 工具 `add_tool({name})`：新增一个工具并发送 tools/list_changed；
//! - 工具 `crash()`：进程立即退出（用于测试 Host 的重启）；
//! - 工具 `typed()`：声明 `outputSchema`（`{n: number}`），但返回不符合的 `structuredContent`（Hub 不核对上游结果的 schema）；
//! - 工具 `blob({bytes})`：返回指定字节数的文本（用于测试结果大小上限）；
//! - 资源 `demo://greeting`；
//! - `instructions` 超过 100 字符，用于测试上游总览。
//!
//! 仅供集成测试使用（`crates/host/tests/upstream_http.rs` 通过 `CARGO_BIN_EXE_app-mcp-test-upstream` 启动）。

use std::sync::{Arc, Mutex};

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListResourcesResult,
    ListToolsResult, PaginatedRequestParams, ProtocolVersion, ReadResourceRequestParams,
    ReadResourceResponse, ReadResourceResult, Resource, ResourceContents, ServerCapabilities,
    ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use serde_json::{Map, Value, json};

#[derive(Clone, Default)]
struct Echo {
    extra: Arc<Mutex<Vec<String>>>,
}

fn schema(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

impl ServerHandler for Echo {
    fn get_info(&self) -> ServerConfig {
        let caps = ServerCapabilities::builder()
            .enable_tools()
            .enable_tool_list_changed()
            .enable_resources()
            .build();
        let instructions = format!("回显服务器：{}。这里是正文部分。", "用于测试".repeat(30));
        ServerConfig::new(caps)
            .with_protocol_version(ProtocolVersion::LATEST_WITH_INITIALIZE)
            .with_instructions(instructions)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let text_schema = json!({"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]});
        let mut tools = vec![
            Tool::new("echo", "原样返回文本", schema(text_schema)),
            Tool::new(
                "add_tool",
                "新增一个工具",
                schema(json!({"type": "object", "properties": {"name": {"type": "string"}}})),
            ),
            Tool::new("crash", "立即退出进程", schema(json!({"type": "object"}))),
            Tool::new("typed", "返回不符合 outputSchema 的结果", schema(json!({"type": "object"})))
                .with_raw_output_schema(Arc::new(schema(json!({
                    "type": "object",
                    "properties": {"n": {"type": "number"}},
                    "required": ["n"]
                })))),
            Tool::new(
                "blob",
                "返回指定字节数的文本",
                schema(json!({"type": "object", "properties": {"bytes": {"type": "integer"}}})),
            ),
        ];
        let extra = self.extra.lock().map(|e| e.clone()).unwrap_or_default();
        for name in extra {
            tools.push(Tool::new(
                name,
                "动态工具",
                schema(json!({"type": "object"})),
            ));
        }
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let args = request.arguments.unwrap_or_default();
        let result = match request.name.as_ref() {
            "echo" => {
                let text = args.get("text").and_then(Value::as_str).unwrap_or_default();
                CallToolResult::success(vec![ContentBlock::text(format!("echo: {text}"))])
            }
            "add_tool" => {
                let name = args
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("dynamic")
                    .to_owned();
                if let Ok(mut e) = self.extra.lock() {
                    e.push(name);
                }
                let _ = context.peer.notify_tool_list_changed().await;
                CallToolResult::success(vec![ContentBlock::text("ok")])
            }
            "crash" => std::process::exit(3),
            "typed" => CallToolResult::structured(json!({"n": "不是数字"})),
            "blob" => {
                let bytes = args.get("bytes").and_then(Value::as_u64).unwrap_or(0);
                let len = usize::try_from(bytes).unwrap_or(0);
                CallToolResult::success(vec![ContentBlock::text("x".repeat(len))])
            }
            other => {
                return Err(ErrorData::invalid_params(
                    format!("unknown tool {other}"),
                    None,
                ));
            }
        };
        Ok(result.into())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult::with_all_items(vec![
            Resource::new("demo://greeting", "greeting").with_description("问候语"),
        ]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        if request.uri != "demo://greeting" {
            return Err(ErrorData::resource_not_found("no such resource", None));
        }
        Ok(ReadResourceResult::new(vec![ResourceContents::text("你好，上游", request.uri)]).into())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let service = Echo::default().serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}
