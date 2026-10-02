//! 不透明句柄的 Rust 侧定义。

use std::collections::HashMap;
use std::ffi::CString;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use app_mcp_native::{
    CallHandle, HoldHandle, NativeClient, NavigateHandle, ReadHandle, ResourceHandle, ScopeHandle, ToolHandle,
};

use crate::status::{FfiError, FfiResult};
use crate::strings::to_cstring_lossy;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 直接注册在根作用域下的项目。原生运行时没有“根 scope”句柄，
/// `am_scope_dispose(root)` 需要逐个注销它们。
#[derive(Default)]
pub(crate) struct RootItems {
    tools: HashMap<String, ToolHandle>,
    resources: HashMap<String, ResourceHandle>,
    scopes: Vec<ScopeHandle>,
}

/// 一个客户端的共享状态。`am_client_free` 之后 `client` 为 `None`，
/// 由该客户端创建的其他句柄据此返回 `AM_ERR_STOPPED`。
pub(crate) struct ClientShared {
    client: RwLock<Option<NativeClient>>,
    root: Mutex<RootItems>,
}

impl ClientShared {
    pub fn client(&self) -> FfiResult<NativeClient> {
        let guard = self.client.read().unwrap_or_else(|e| e.into_inner());
        guard.clone().ok_or_else(FfiError::stopped)
    }

    pub fn check_alive(&self) -> FfiResult<()> {
        let guard = self.client.read().unwrap_or_else(|e| e.into_inner());
        if guard.is_some() {
            Ok(())
        } else {
            Err(FfiError::stopped())
        }
    }

    pub fn track_tool(&self, h: &ToolHandle) {
        lock(&self.root).tools.insert(h.name(), h.clone());
    }

    pub fn track_resource(&self, h: &ResourceHandle) {
        lock(&self.root).resources.insert(h.name(), h.clone());
    }

    pub fn track_scope(&self, h: &ScopeHandle) {
        lock(&self.root).scopes.push(h.clone());
    }

    /// 注销根作用域下的全部项目。
    pub fn dispose_root(&self) {
        let items = std::mem::take(&mut *lock(&self.root));
        for s in items.scopes {
            s.dispose();
        }
        for t in items.tools.into_values() {
            t.dispose();
        }
        for r in items.resources.into_values() {
            r.dispose();
        }
    }

    /// 停止并释放客户端。
    pub fn shutdown(&self) {
        let client = self
            .client
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        // 丢弃根作用域下的句柄（不注销），避免它们延长运行时的生命周期。
        drop(std::mem::take(&mut *lock(&self.root)));
        if let Some(c) = client {
            c.stop();
            drop(c);
        }
    }
}

pub struct AmClient {
    pub(crate) shared: Arc<ClientShared>,
}

impl AmClient {
    pub(crate) fn new(client: NativeClient) -> Self {
        Self {
            shared: Arc::new(ClientShared {
                client: RwLock::new(Some(client)),
                root: Mutex::new(RootItems::default()),
            }),
        }
    }
}

pub(crate) enum ScopeKind {
    Root,
    Child(ScopeHandle),
}

pub struct AmScope {
    pub(crate) kind: ScopeKind,
    pub(crate) shared: Arc<ClientShared>,
}

pub struct AmTool {
    pub(crate) handle: ToolHandle,
    pub(crate) shared: Arc<ClientShared>,
}

pub struct AmResource {
    pub(crate) handle: ResourceHandle,
    pub(crate) shared: Arc<ClientShared>,
}

/// 一次调用；缓存的 C 字符串在 call 被消费前有效。
pub struct AmCall {
    pub(crate) handle: CallHandle,
    pub(crate) id: CString,
    pub(crate) tool_name: CString,
    pub(crate) arguments: CString,
    /// v16：Agent 幂等键（spec/protocol.md 3.3）；没有时为 None。
    pub(crate) idempotency_key: Option<CString>,
}

impl AmCall {
    pub(crate) fn new(handle: CallHandle) -> Self {
        let id = to_cstring_lossy(&handle.call_id());
        let tool_name = to_cstring_lossy(&handle.tool_name());
        let arguments = to_cstring_lossy(&handle.arguments_json());
        let idempotency_key = handle.idempotency_key().map(|k| to_cstring_lossy(&k));
        Self {
            handle,
            id,
            tool_name,
            arguments,
            idempotency_key,
        }
    }
}

pub struct AmRead {
    pub(crate) handle: ReadHandle,
    pub(crate) resource_name: CString,
}

impl AmRead {
    pub(crate) fn new(handle: ReadHandle) -> Self {
        let resource_name = to_cstring_lossy(&handle.resource_name());
        Self {
            handle,
            resource_name,
        }
    }
}

/// 一次导航请求（v14）。由 `am_navigate_complete` / `_fail` / `_deny` 消费。
pub struct AmNavigate {
    pub(crate) handle: NavigateHandle,
    pub(crate) page: CString,
    pub(crate) params_json: Option<CString>,
}

impl AmNavigate {
    pub(crate) fn new(handle: NavigateHandle) -> Self {
        let page = to_cstring_lossy(&handle.page());
        let params_json = handle.params_json().map(|p| to_cstring_lossy(&p));
        Self { handle, page, params_json }
    }
}

/// 阻止自动休眠的持有（v3）。`am_hold_release` 释放持有并释放句柄。
pub struct AmHold {
    pub(crate) handle: HoldHandle,
}
