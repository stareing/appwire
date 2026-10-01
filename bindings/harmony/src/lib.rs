//! 鸿蒙（HarmonyOS NEXT / OpenHarmony）Node-API 模块：把 `app-mcp-native` 暴露给 ArkTS
//! （`sdks/harmony` 的 `@app-mcp/harmony` 在其上封装 ArkTS API）。
//!
//! ArkTS 运行时（ArkVM）实现 Node-API 兼容接口（`libace_napi.z.so`），因此 JS 侧 API 与
//! `bindings/node`（napi-rs）**逐字相同**：本 crate 用 napi-rs 的 OpenHarmony 分支 napi-ohos
//! 编译同一份源码（`bindings/node/src/lib.rs`），不复制实现。两者的 crate 名不同
//! （`napi` ↔ `napi_ohos`、`napi_derive` ↔ `napi_derive_ohos`），这里在 crate 根以
//! `extern crate … as …` 统一名字；宏展开出的路径本身指向 `napi_ohos`。
//!
//! 默认端点不需要鸿蒙专有代码：`ClientConfig.hostUrl` 缺省时 `app-mcp-native` 按 spec/protocol.md 1.3 解析，
//! `app_mcp_protocol::platform` 把 `target_env = "ohos"` 归为应用沙箱（无默认本地 IPC、不读登记文件），
//! 结果为 `ws://127.0.0.1:7717/app`（开发机上的 Host 经 `hdc rport tcp:7717 tcp:7717` 反向转发）。
//!
//! @compat 共享源码中的 napi-rs API 必须在 napi-ohos 中存在；bindings/node 改用 napi-ohos 尚未提供的
//! API 时本 crate 编译失败（`cargo check` 即可发现）。
//! @why 线程模型：回调经 ThreadsafeFunction 投递到 ArkTS 主线程（创建 NativeClient 的线程）的事件循环。

extern crate napi_derive_ohos as napi_derive;
extern crate napi_ohos as napi;

#[path = "../../node/src/lib.rs"]
mod shared;

pub use shared::*;
