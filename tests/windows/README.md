# Windows 实机验证

在 Windows 上（或从 WSL 通过 interop 调用 Windows 程序）验证唤醒链路。产物放 `target\win`，与 Linux 的 `target\debug` 分开。

```bat
:: 1. Rust（MSVC）：Host 与 C ABI
set CARGO_TARGET_DIR=D:\code-work\tastyrice\target\win
cargo build -p app-mcp-host -p app-mcp-c

:: 2. 测试 App（net9.0，RollForward=Major；--artifacts-path 避免覆盖 Linux 的 obj/）
dotnet build tests\windows\WakeApp\WakeApp.csproj --artifacts-path target\win\dotnet

:: 3. 端到端（a 冷启动 / b 休眠唤醒 / c web-url / d aumid，可只跑部分：node ... a b）
node tests\windows\wake-e2e.mjs
```

- `WakeApp`：无窗口（WinExe）的 C# App，`SingleInstance` 转发 + idle 模式（空闲 3 秒休眠），
  工具 `echo` 返回进程 PID；日志写程序目录下 `wakeapp.log`。`--register` / `--unregister` 写 / 删
  `HKCU\Software\Classes\appmcp-wintest`，`--quit` 让运行中的实例退出。
- `wake-e2e.mjs`：启动 `app-mcp-host.exe`（WebSocket 127.0.0.1:7791，租约 1 秒，唤醒超时 12 秒）与三个清单
  （`wintest` uri、`webtest` web-url 指向本地记录服务器、`calctest` 计算器 AUMID），以 stdio JSON-RPC 调用工具并按
  Host / App 日志判定。结束时结束测试进程（计算器只结束测试新启动的那个）、删除注册表键。
- d 由 Host 以 `IApplicationActivationManager::ActivateApplication(<计算器 AUMID>, "app-mcp-wake:<令牌>")` 激活。
  计算器自身不接受启动参数（返回 0x80040904/5 并退出），因此 d 只判定 Host 以令牌参数发起了激活；
  激活链路本身由 `cargo test -p app-mcp-hub -- --ignored activate_calculator`（空参数，返回 PID 后结束进程）验证。
  用本仓库的测试 App 做打包（MSIX）版本的 d 需要签名证书 + 开发者模式 / 旁加载，本机未开启，未做。
- c 会在默认浏览器中打开一个 `http://127.0.0.1:<端口>/wake` 标签页；页面回报令牌后尝试 `window.close()`，
  Chrome 不一定允许关闭，需手动关掉。

## 按名寻址（spec/naming.md 4.3，TASKS 4d-E）

```bat
set CARGO_TARGET_DIR=D:\code-work\tastyrice\target\win
:: 库级 e2e：临时登记目录 + exec 激活 + 示例 App named_app（含 CHANNEL_LIMIT、冒名管道、目录通知、程序缺失）
cargo test -p app-mcp-hub --test naming_pipe -- --nocapture
:: Host 级 e2e：app install → stdio Host --name-service → 冷启动 → 宽限后退出 → 再激活 → app uninstall
cargo build -p app-mcp-host && cargo build -p app-mcp-native --example named_app
node tests\windows\naming-e2e.mjs
```

- `naming-e2e.mjs` 写真实的 `%LOCALAPPDATA%\app-mcp\apps\naming-wintest-<随机>.json`，结束时 `app uninstall` 删除；Host 用临时
  `--home`、`--listen 127.0.0.1:0`、`--ipc-endpoint none`、`--waker none`（激活只经名字服务）。
- 管道是否存在用 PowerShell 枚举 `\\.\pipe\` 观察（仅测试与人工排查；Hub 不枚举，U-06）。
