//! napi-rs 绑定：把 `app-mcp-native` 暴露给 Node（`@app-mcp/node` 在其上封装 JS API）。
//!
//! # 线程模型
//!
//! 原生运行时在自己的分发线程上触发回调（工具调用、资源读取、取消、状态、配对、日志）。
//! 本绑定把每个 JS 回调包装成 [`ThreadsafeFunction`]，在分发线程上以非阻塞方式投递到
//! Node 事件循环，回调本身在 JS 主线程执行。JS 完成 handler 后调用 `Call.complete` /
//! `Call.fail`（可在任意时刻、任意次数尝试，重复完成返回 `ALREADY_COMPLETED` 错误）。
//!
//! # 进程退出
//!
//! 所有 ThreadsafeFunction 都以 **weak** 模式创建（`napi_unref_threadsafe_function`），
//! 不会阻止 Node 进程退出；是否保持进程存活由 JS 封装层决定（`keepAlive` 选项）。
//! ThreadsafeFunction 在最后一个 Rust 引用被丢弃时释放：工具 / 资源注销后原生运行时丢弃
//! handler，`stop()` 时丢弃客户端监听器。
//!
//! # 错误
//!
//! 所有方法抛出的 JS `Error` 的 `code` 为 [`NativeError`](app_mcp_native::NativeError) 对应的大写代码
//! （如 `DUPLICATE_NAME`、`STOPPED`），参数不合法时为 `INVALID_ARG`。

#![deny(clippy::all)]

mod callbacks;
mod client;
mod convert;
mod handles;
mod objects;

use napi::Status;
use napi::bindgen_prelude::Unknown;
use napi::threadsafe_function::ThreadsafeFunction;

pub use client::JsNativeClient;
pub use handles::{Call, Hold, Navigate, Read, Resource, Scope, Tool};
pub use objects::{
    CallDedupInit, CallResultInit, ClientConfig, ClientEvent, ContentAnnotationsInit, DeprecationInit, JsStateInfo, LifecycleInit,
    OverviewInit, ResourceSpecInit, ToolAnnotationsInit, ToolSpecInit, WakeInit,
};

/// 不阻止进程退出（weak）、不带 error-first 参数（callee_handled = false）的 ThreadsafeFunction。
type WeakTsfn<T> = ThreadsafeFunction<T, Unknown<'static>, T, Status, false, true>;
