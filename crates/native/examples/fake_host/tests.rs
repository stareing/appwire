use super::*;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn parses_interleaved_ops() {
    let o = parse_args(args(&[
        "--addr",
        "127.0.0.1:9",
        "--invoke",
        "a",
        "--args",
        r#"{"x":1}"#,
        "--read",
        "r",
        "--invoke",
        "b",
        "--timeout-ms",
        "50",
    ]))
    .unwrap();
    assert_eq!(o.addr.as_deref(), Some("127.0.0.1:9"));
    assert_eq!(o.ipc, None);
    assert_eq!(o.timeout_ms, 50);
    assert_eq!(
        o.ops,
        vec![
            Op::invoke("a", json!({"x": 1})),
            Op::Read { name: "r".into() },
            Op::invoke("b", json!({})),
        ]
    );
}

#[test]
fn parses_lifecycle_ops() {
    let o = parse_args(args(&[
        "--invoke", "a", "--await-sleep", "--wake", "--read", "r", "--await-sleep",
        "--lease-ms", "100", "--reject-sleep", "50",
    ]))
    .unwrap();
    assert_eq!(o.lease_ms, Some(100));
    assert_eq!(o.reject_sleep_ms, Some(50));
    assert_eq!(
        o.ops,
        vec![
            Op::invoke("a", json!({})),
            Op::AwaitSleep,
            Op::Wake,
            Op::Read { name: "r".into() },
            Op::AwaitSleep,
        ]
    );
    assert!(parse_args(args(&["--wake"])).is_err());
    assert!(parse_args(args(&["--invoke", "a", "--wake"])).is_err());
    assert!(parse_args(args(&["--lease-ms", "x"])).is_err());
}

#[test]
fn parses_conformance_ops() {
    let o = parse_args(args(&[
        "--trace", "--invoke", "a", "--call-id", "x", "--invoke-timeout-ms", "100", "--cancel-after-ms", "50",
        "--no-wait", "--catalog", "200", "--delay", "10", "--sdk", "rust", "--report-dir", "out",
    ]))
    .unwrap();
    assert!(o.trace);
    assert_eq!(o.sdk.as_deref(), Some("rust"));
    assert_eq!(
        o.ops,
        vec![
            Op::Invoke {
                name: "a".into(),
                args: json!({}),
                opts: InvokeOpts {
                    call_id: Some("x".into()),
                    timeout_ms: Some(100),
                    cancel_after_ms: Some(50),
                    idempotency_key: None,
                    no_wait: true,
                },
            },
            Op::Catalog { settle_ms: 200 },
            Op::Delay { ms: 10 },
        ]
    );
    assert!(parse_args(args(&["--call-id", "x"])).is_err());
    let o = parse_args(args(&["--navigate", "cart", "--nav-params", r#"{"id":1}"#, "--navigate", "x"])).unwrap();
    assert_eq!(
        o.ops,
        vec![
            Op::Navigate { page: "cart".into(), params: Some(json!({"id": 1})) },
            Op::Navigate { page: "x".into(), params: None },
        ]
    );
    assert!(parse_args(args(&["--nav-params", "{}"])).is_err());
    assert!(parse_args(args(&["--read", "r", "--cancel-after-ms", "5"])).is_err());
    assert!(parse_args(args(&["--read", "r", "--no-wait"])).is_err());
}

#[test]
fn rejects_bad_args() {
    assert!(parse_args(args(&["--args", "{}"])).is_err());
    assert!(parse_args(args(&["--invoke", "a", "--args", "nope"])).is_err());
    assert!(parse_args(args(&["--invoke"])).is_err());
    assert!(parse_args(args(&["--bogus"])).is_err());
    assert!(parse_args(args(&["--timeout-ms", "x"])).is_err());
}

#[test]
fn parses_ipc_endpoint() {
    let o = parse_args(args(&["--ipc", "unix:/tmp/x/hub.sock"])).unwrap();
    assert_eq!(o.ipc, Some(Endpoint::unix("/tmp/x/hub.sock")));
    assert_eq!(o.addr, None);
    let o = parse_args(args(&["--ipc", r"pipe:\\.\pipe\fake-1"])).unwrap();
    assert_eq!(o.ipc, Some(Endpoint::Pipe(r"\\.\pipe\fake-1".to_owned())));
    // 非 IPC 端点、非法端点、与 --addr 同用都拒绝。
    assert!(parse_args(args(&["--ipc", "ws://127.0.0.1:1/app"])).is_err());
    assert!(parse_args(args(&["--ipc", "unix:relative.sock"])).is_err());
    assert!(parse_args(args(&["--addr", "127.0.0.1:0", "--ipc", "unix:/tmp/a.sock"])).is_err());
}

/// 经本地 IPC 完成握手并调用一次工具（Unix 套接字放在临时目录、Windows 用每进程独立管道名，不碰默认端点）。
#[tokio::test(flavor = "current_thread")]
async fn serves_over_ipc() {
    use app_mcp_protocol::endpoint::IPC_WS_URL;
    #[cfg(unix)]
    let dir = std::env::temp_dir().join(format!("fake-host-test-{}", std::process::id()));
    #[cfg(unix)]
    let endpoint = {
        std::fs::create_dir_all(&dir).unwrap();
        format!("unix:{}", dir.join("h.sock").display())
    };
    #[cfg(windows)]
    let endpoint = format!(r"pipe:\\.\pipe\fake-host-test-{}", std::process::id());
    let opts = parse_args(args(&["--ipc", &endpoint, "--invoke", "t"])).unwrap();
    let (listener, shown) = Listener::bind(&opts).await.unwrap();
    assert_eq!(shown, endpoint);
    let host = tokio::spawn(async move { run(listener, opts).await });

    #[cfg(unix)]
    let stream = tokio::net::UnixStream::connect(dir.join("h.sock")).await.unwrap();
    #[cfg(windows)]
    let stream = tokio::net::windows::named_pipe::ClientOptions::new()
        .open(&endpoint["pipe:".len()..])
        .unwrap();
    let (mut ws, _) = tokio_tungstenite::client_async(IPC_WS_URL, stream).await.unwrap();
    let hello = Message::request(RequestId::Number(1), method::HELLO, json!({}));
    ws.send(WsMessage::text(hello.to_json())).await.unwrap();
    let reply = ws.next().await.unwrap().unwrap().into_text().unwrap();
    assert!(reply.contains("fake-"), "{reply}");
    ws.send(WsMessage::text(Message::notification(method::READY, json!({})).to_json()))
        .await
        .unwrap();
    let invoke = Message::parse(&ws.next().await.unwrap().unwrap().into_text().unwrap()).unwrap();
    let Message::Request(req) = invoke else { panic!("应为 tools/invoke 请求") };
    assert_eq!(req.method, method::TOOLS_INVOKE);
    ws.send(WsMessage::text(Message::result(req.id, json!({"data": 1})).to_json()))
        .await
        .unwrap();
    // fake host 完成后关闭连接。
    while let Some(Ok(_)) = ws.next().await {}
    host.await.unwrap().unwrap();
    #[cfg(unix)]
    {
        assert!(!dir.join("h.sock").exists(), "退出后应删除套接字文件");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
