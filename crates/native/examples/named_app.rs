//! 按名寻址的最小 Rust App（spec/naming.md）：在名字服务登记、不主动连接 Hub，由 Hub 拨号时接受通道；
//! 通道关闭后若本进程由激活启动（`--app-mcp-activation`）就退出，进程交还系统。
//!
//! ```text
//! cargo run -p app-mcp-native --example named_app            # 用户直接运行：登记名字，常驻直到 Ctrl-C
//! app-mcp-host app install --app-id named-demo --exec <本程序路径>   # 生成 D-Bus 激活文件后可被按需拉起
//! ```
//!
//! 环境变量：`APP_MCP_APP_ID`（默认 `named-demo`）；`APP_MCP_EVENT_LOG`（可选，追加 `start <pid>` / `exit <pid>` 行，
//! 测试用来核对激活次数与退出）。工具：`echo`（原样返回参数）、`pid`（返回进程号）。

use std::io::Write;
use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};

use app_mcp_native::{
    CallHandle, ClientListener, LifecycleMode, LogLevel, NativeClient, NativeConfig, Residency, StateInfo, ToolHandler,
    ToolSpec,
};

fn note(event: &str) {
    if let Some(path) = std::env::var_os("APP_MCP_EVENT_LOG")
        && let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path)
    {
        let _ = writeln!(f, "{event} {}", std::process::id());
    }
}

struct Echo;
impl ToolHandler for Echo {
    fn invoke(&self, call: CallHandle) {
        let args = call.arguments_json();
        let _ = call.complete(Some(&format!(r#"{{"echo":{args}}}"#)), vec![]);
    }
}

struct Pid;
impl ToolHandler for Pid {
    fn invoke(&self, call: CallHandle) {
        let _ = call.complete(Some(&format!(r#"{{"pid":{}}}"#, std::process::id())), vec![]);
    }
}

struct Listener(std::sync::Mutex<Sender<()>>);
impl ClientListener for Listener {
    fn on_state_changed(&self, _state: StateInfo) {}
    fn on_paired(&self, _token: String) {}
    fn on_log(&self, level: LogLevel, message: String) {
        eprintln!("[named_app] {level:?} {message}");
    }
    fn on_idle_exit(&self) {
        let _ = self.0.lock().map(|tx| tx.send(()));
    }
}

fn main() {
    note("start");
    let app_id = std::env::var("APP_MCP_APP_ID").unwrap_or_else(|_| "named-demo".to_owned());
    let mut cfg = NativeConfig::new(app_id, "按名寻址示例");
    cfg.lifecycle.mode = LifecycleMode::OnDemand;
    cfg.lifecycle.residency = Residency::ExitWhenIdle;
    cfg.register_name = true;
    let (tx, rx) = channel();
    let client = match NativeClient::new(cfg, Some(Arc::new(Listener(std::sync::Mutex::new(tx))))) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[named_app] 配置无效：{e}");
            std::process::exit(2);
        }
    };
    let tools: [(&str, &str, Arc<dyn ToolHandler>); 2] =
        [("echo", "原样返回参数", Arc::new(Echo)), ("pid", "返回进程号", Arc::new(Pid))];
    for (name, desc, handler) in tools {
        if let Err(e) = client.register_tool(ToolSpec::new(name, desc), handler) {
            eprintln!("[named_app] 注册工具 {name} 失败：{e}");
            std::process::exit(2);
        }
    }
    client.start();
    // 由激活启动：通道关闭后收到 on_idle_exit 即退出；用户直接运行时不会收到，一直等待。
    let _ = rx.recv();
    client.stop();
    drop(client);
    note("exit");
}
