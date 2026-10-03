//! uniffi 绑定（proc-macro 方式）：把 `app-mcp-native` 暴露给 Kotlin、Swift、Python。
//!
//! # 设计
//!
//! - 对象（`uniffi::Object`）：[`AppMcpClient`]、[`Scope`]、[`Tool`]、[`Resource`]、[`Call`]、[`Read`]、
//!   [`Hold`]，分别包装原生运行时的 `NativeClient` 与各类句柄。
//! - 记录（`uniffi::Record`）：[`ClientConfig`]、[`AppOverview`]、[`ToolSpec`]、[`ToolAnnotations`]、[`ResourceSpec`]、[`EventInfo`]、
//!   [`CallResult`]、[`ContentAnnotations`]、[`StateInfo`]、[`LifecyclePolicy`]、[`WakeDescriptor`]、[`CallDedupPolicy`]。
//! - 枚举（`uniffi::Enum`）：[`Risk`]、[`ResultStatus`]、[`Audience`]、[`Activation`]、[`Visibility`]、[`ClientKind`]、[`CancelReason`]、
//!   [`StateStatus`]、[`LogLevel`]、[`LifecycleMode`]、[`Residency`]、[`WakeKind`]、[`WakeReason`]、[`SleepReason`]、
//!   [`ChannelOffer`]。
//! - 函数：[`error_kinds`]、[`parse_wake_token`]。
//! - 错误（`uniffi::Error`）：[`AppMcpError`]。
//! - 由外部语言实现的回调接口（`#[uniffi::export(foreign)]`，即旧写法 `with_foreign` 的规范形式）：
//!   [`ToolHandler`]、[`ResourceReader`]、[`CancelListener`]、[`ClientListener`]。
//!
//! # 线程模型
//!
//! 与原生运行时一致，**回调是同步的**：`ToolHandler::invoke` 等在原生运行时的分发线程上调用，
//! 必须尽快返回。语言封装层在回调里把工作切换到合适的线程（UI 线程、线程池、协程），
//! 之后从任意线程调用 [`Call::complete`] / [`Call::fail`] 完成调用。
//!
//! 外部回调抛出的未预期异常在 uniffi 里会变成 panic；这里的适配器用 `catch_unwind` 兜底，
//! 工具调用转为 `HANDLER_ERROR`，其余回调忽略，保证不会让分发线程崩溃。
//!
//! # 错误类别
//!
//! 在 FFI 上用协议字符串表示（如 `"HANDLER_ERROR"`、`"INVALID_INPUT"`），见 [`error_kinds`]。


uniffi::setup_scaffolding!();

mod enums;
mod error;
mod config;
mod records;
mod callbacks;
mod handles;
mod client;

pub use enums::*;
pub use error::*;
pub use config::*;
pub use records::*;
pub use callbacks::*;
pub use handles::*;
pub use client::*;

#[cfg(test)]
mod tests;
