//! 鸿蒙（HarmonyOS NEXT / OpenHarmony）Node-API 模块：把 `app-mcp-native` 暴露给 ArkTS
//! （`sdks/harmony` 的 `@app-mcp/harmony` 在其上封装 ArkTS API）。
//!
//! ArkTS 运行时（ArkVM）实现 Node-API 兼容接口（`libace_napi.z.so`），因此 JS 侧 API 与
//! `bindings/node`（napi-rs）**逐字相同**：本 crate 用 napi-rs 的 OpenHarmony 分支 napi-ohos
//! 编译同一份源码（`bindings/node/src/lib.rs`），不复制实现。两者的 crate 名不同
//! （`napi` ↔ `napi_ohos`、`napi_derive` ↔ `napi_derive_ohos`），这里在 crate 根以
//! `extern crate … as …` 统一名字；宏展开出的路径本身指向 `napi_ohos`。
//!
//! 鸿蒙专有的部分只在本文件：
//! - [`default_host_url`]：ArkTS 应用沙箱内没有可用的本地 IPC 端点（与 Android 相同），
//!   封装层未指定 `hostUrl` 时用它（开发机上的 Host 经 `hdc rport tcp:7717 tcp:7717` 反向转发）。
//!
//! @compat 共享源码中的 napi-rs API 必须在 napi-ohos 中存在；bindings/node 改用 napi-ohos 尚未提供的
//! API 时本 crate 编译失败（`cargo check` 即可发现）。
//! @why 线程模型：回调经 ThreadsafeFunction 投递到 ArkTS 主线程（创建 NativeClient 的线程）的事件循环。

extern crate napi_derive_ohos as napi_derive;
extern crate napi_ohos as napi;

#[path = "../../node/src/lib.rs"]
mod shared;

pub use shared::*;

use napi_derive_ohos::napi;

/// ArkTS 封装层的默认 Host 端点：`ws://127.0.0.1:7717/app`（spec/protocol.md 第 1 节）。
///
/// @why `NativeConfig::new` 的默认端点按 `target_os` 选择本地 IPC；ohos 目标的 `target_os` 是 `linux`，
/// 会得到桌面 Linux 的套接字路径，而应用沙箱内不存在该路径，故由封装层显式传入。
#[napi]
pub fn default_host_url() -> String {
    app_mcp_protocol::DEFAULT_WS_URL.to_owned()
}
