//! Tauri MockRuntime：经真实 IPC 命令 `plugin:app-mcp|op`（含 ACL）登记。

use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{
    INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder, mock_context, noop_assets,
};
use tauri::utils::acl::ExecutionContext;
use tauri::webview::InvokeRequest;
use tauri::{App, WebviewWindow, WebviewWindowBuilder};

use super::*;

fn app(plugin: TauriPlugin<MockRuntime>) -> App<MockRuntime> {
    let mut context = mock_context(noop_assets());
    context
        .runtime_authority_mut()
        .__allow_command("plugin:app-mcp|op".into(), ExecutionContext::Local);
    mock_builder()
        .plugin(plugin)
        .build(context)
        .expect("构建 App")
}

fn invoke(webview: &WebviewWindow<MockRuntime>, op: Value) -> Value {
    let request = InvokeRequest {
        cmd: "plugin:app-mcp|op".into(),
        callback: CallbackFn(0),
        error: CallbackFn(1),
        url: if cfg!(any(windows, target_os = "android")) {
            "http://tauri.localhost"
        } else {
            "tauri://localhost"
        }
        .parse()
        .expect("url"),
        body: InvokeBody::Json(json!({ "op": op })),
        headers: Default::default(),
        invoke_key: INVOKE_KEY.to_string(),
    };
    match get_ipc_response(webview, request) {
        Ok(body) => body.deserialize::<Value>().expect("应答 JSON"),
        Err(e) => panic!("IPC 失败：{e}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_registers_page_and_rust_tools_over_tauri_ipc() {
    let (hub, ep) = start_hub("mock").await;
    let app = app(Builder::new(native_config(&ep, "mock"))
        .wake_from_args(false)
        .build());
    let app_mcp = app.app_mcp().expect("插件状态");
    assert_eq!(app_mcp.page_count(), 0);

    struct Ping;
    impl ToolHandler for Ping {
        fn invoke(&self, call: CallHandle) {
            let _ = call.complete(Some("\"pong\""), vec![]);
        }
    }
    app_mcp
        .client()
        .register_tool(ToolSpec::new("app.ping", "Rust 工具"), Arc::new(Ping))
        .expect("注册");

    let main = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("窗口");
    let hello = invoke(&main, json!({ "op": "hello" }));
    assert_eq!(hello["ok"], true);
    assert_eq!(hello["value"]["instanceId"], app_mcp.client().instance_id());
    assert_eq!(
        invoke(&main, register(1, "page.tool")),
        json!({ "ok": true })
    );
    assert_eq!(app_mcp.page_count(), 1);

    eventually("Hub 看到 Rust 与页面工具", || {
        tool_names(&hub, "mock") == vec!["app.ping", "page.tool"]
    })
    .await;
    let apps = hub.apps();
    assert!(apps.iter().any(|a| a.app_id == "mock" && a.connected));
    let out = hub
        .call_tool(CallRequest::new("mock.app.ping", json!({})))
        .await
        .expect("调用");
    assert_eq!(out.result.expect("成功"), json!("pong"));

    // 可见性：按窗口状态上报（MockRuntime 的窗口可见、未聚焦），重复上报被去重。
    app_mcp.refresh_visibility();
    assert_eq!(
        *app_mcp
            .last_visibility
            .lock()
            .unwrap_or_else(|p| p.into_inner()),
        Some((Visibility::Visible, false))
    );

    // 页面卸载。
    assert_eq!(
        invoke(&main, json!({ "op": "reset" })),
        json!({ "ok": true })
    );
    eventually("页面工具注销", || {
        tool_names(&hub, "mock") == vec!["app.ping"]
    })
    .await;

    // 退出：停止客户端。
    invoke(&main, register(2, "page.again"));
    app_mcp.on_run_event(&RunEvent::Exit);
    assert_eq!(app_mcp.page_count(), 0);
    assert_eq!(app_mcp.client().state().status, StateStatus::Stopped);
    shutdown(hub).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn forbidden_webview_and_wake_args() {
    let (hub, ep) = start_hub("mockfilter").await;
    let app = app(Builder::new(native_config(&ep, "mockfilter"))
        .accept_webview(|label| label == "main")
        .auto_start(false)
        .wake_from_args(false)
        .build());
    let other = WebviewWindowBuilder::new(&app, "other", Default::default())
        .build()
        .expect("窗口");
    assert_eq!(
        invoke(&other, json!({ "op": "hello" }))["code"],
        "FORBIDDEN"
    );

    let app_mcp = app.app_mcp().expect("插件状态");
    assert!(!app_mcp.handle_wake_args(["--flag", "/some/path"]));
    // 唤醒令牌：on-demand 之外也会被识别（冷启动唤醒）。
    assert!(
        app_mcp.handle_wake_args(["--flag", "app-mcp-wake:0123456789abcdef0123456789abcdef"])
    );
    app_mcp.client().stop();
    shutdown(hub).await;
}
