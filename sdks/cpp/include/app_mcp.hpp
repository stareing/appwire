// app_mcp.hpp —— app-mcp 的 C++17 header-only RAII 封装（基于 app_mcp.h）。
//
// 线程模型：handler、资源读取与客户端事件都在库的分发线程上调用，必须尽快返回。
// 需要在 UI 线程执行的工作，把 Call / Read 移动到 UI 线程后再完成（complete / fail 可在任意线程调用）。
// 例如 Qt：QMetaObject::invokeMethod(obj, [c = std::make_shared<Call>(std::move(call))] { ... });
//
// 所有权：
// - Client / Scope / Tool / Resource 析构时释放句柄，但不会注销工具或资源（注销用 dispose()）；
//   Client 析构会停止客户端。
// - Call / Read 为只能移动的对象，必须完成一次；析构时若尚未完成，以 HANDLER_ERROR 失败。
// - handler 抛出 ToolCallError 时按其 kind（及可选 details）失败，抛出其他 std::exception 时以 HANDLER_ERROR 失败。
//   需要用户本人操作（登录过期、权限未授予、需切到前台等）时抛出 UserActionRequired 或调用 Call::fail_user_action。
//
// 生命周期（spec/lifecycle.md，需要 AM_API_VERSION >= 3）：
// - ClientConfig::lifecycle 设置 persistent / idle / on-demand；空闲后与 Host 完成 app/sleep 并释放连接与运行时。
// - Client::handle_wake(args) 处理操作系统激活参数（命令行、URL、D-Bus action 参数），识别后回连。
// - Client::hold() / Call::hold() 返回 HoldGuard，析构时释放（RAII），期间不会自动休眠。
// - ClientCallbacks::on_idle_exit：residency 允许时，休眠完成后回调，App 自行决定是否退出。
// - 4e 功耗（spec/lifecycle.md 第 11、13 节）：ClientConfig::heartbeat、Lifecycle::host_absent_retries /
//   legacy_timers / merge_window_ms / sleep_on_background、ResourceOptions::realtime。
//   本封装不区分平台，默认 persistent（核心兼容）；平台默认（手机 on-demand、桌面 idle）由 App 自行设置。
//
// 工具声明与调用结果（spec/protocol.md 第 3 节、3.2；app_mcp.h v9）：
// - ToolOptions::annotations（标准 MCP 工具注解）、ToolOptions::output_schema_json（MCP outputSchema）；
//   ToolOptions::risk 为旧写法，优先用 annotations（同时声明时注解中的字段优先）。
// - Call::complete(const CallResult&)：业务状态（done / pending / partial / noop）、state_resource、summary、内容注解。
// - ResourceOptions::annotations（资源内容的标注，app_mcp.h v13）；ClientConfig::call_dedup（调用去重，v13）。
//
// 按名寻址（spec/naming.md，app_mcp.h v17）：ClientConfig::register_name / name_instance。App 在系统名字服务登记，
// 不主动连接 Hub；由激活启动（命令行带 --app-mcp-activation）时通道关闭后触发 on_idle_exit，示例见 examples/named.cpp。
//
// 界面级暴露与导航（spec/protocol.md 3.4，app_mcp.h v14）：
// - ToolOptions::surface（Surface::App 缺省 / Surface::View 依赖界面）与 ToolOptions::page（所在页面）。
//   view 工具只在所在界面可见且处于最上层时注册（或 set_enabled(true)）；何时可见由 App / UI 框架决定。
// - Client::set_navigation_handler(handler)：Host 的 app/navigate 交给 handler(Navigate)，在分发线程上调用；
//   handler 把 Navigate 移动到 UI 线程，切换页面（新页面的工具注册之后）再 complete()，不愿切换时 deny(message)，
//   出错时 fail(message)。handler 抛出 NavigationDenied → 拒绝，其他异常 → 失败。能力在握手时声明，start() 之前设置。
// - 后台时（app_mcp.h v15，spec/protocol.md 3.4「后台与前台」）：需要前台的导航立即以 USER_ACTION_REQUIRED
//   （reason "foreground"）返回；后台也要能用的能力做成 app 工具，或给 view 工具声明 ToolOptions::background_tool。
//   Client::set_navigate_in_background(true)（桌面默认）时后台导航仍交给 handler：可自行把窗口提到前台，或
//   Navigate::fail_user_action / 抛出 UserActionRequired 回复 USER_ACTION_REQUIRED。
#ifndef APP_MCP_HPP
#define APP_MCP_HPP

#include "app_mcp/errors.hpp"
#include "app_mcp/types.hpp"
#include "app_mcp/handlers.hpp"
#include "app_mcp/client.hpp"

#endif  // APP_MCP_HPP
