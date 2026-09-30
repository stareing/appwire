//! 从环境变量读取 launch token。单独一个测试二进制，避免与其他测试并发修改环境变量。

mod common;

use app_mcp_native::{NativeClient, NativeConfig};
use app_mcp_protocol::method;
use common::MockHost;

#[test]
fn launch_token_from_env() {
    let host = MockHost::start();
    // SAFETY：本测试二进制只有这一个测试，设置时没有其他线程读取环境变量。
    unsafe { std::env::set_var("APP_MCP_LAUNCH_TOKEN", "launch-xyz") };
    let client = NativeClient::new(
        {
            let mut c = NativeConfig::new("test-app", "测试应用");
            c.host_url = host.url();
            c.instance_id = Some("inst-1".into());
            c
        },
        None,
    )
    .unwrap();
    unsafe { std::env::remove_var("APP_MCP_LAUNCH_TOKEN") };
    client.start();
    let hello = host.wait_request(method::HELLO);
    assert_eq!(hello["launchToken"], "launch-xyz");
    assert_eq!(hello["instanceId"], "inst-1");
}
