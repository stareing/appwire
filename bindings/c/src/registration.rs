//! Scope、工具与资源的注册、更新与释放。

use std::ffi::{c_char, c_void};
use std::sync::Arc;

use crate::callbacks::{AmFreeFn, AmReadFn, AmToolFn, CResourceReader, CToolHandler, UserData};
use crate::convert::{
    consume, convert_resource_spec, convert_tool_spec, prepare_out, read_resource_options, read_tool_options,
};
use crate::ffi_types::{AmResourceOptions, AmResourceSpec, AmToolOptions, AmToolSpec};
use crate::handles::{AmResource, AmScope, AmTool, ScopeKind};
use crate::status::{AmStatus, FfiError, FfiResult, guard};
use crate::strings::req_str;

// ---------------------------------------------------------------------------
// Scope
// ---------------------------------------------------------------------------

pub(crate) unsafe fn scope_ref<'a>(scope: *const AmScope) -> FfiResult<&'a AmScope> {
    // SAFETY: scope 为 NULL 或有效句柄。
    unsafe { scope.as_ref() }.ok_or_else(|| FfiError::null("scope"))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_scope_create(
    parent: *mut AmScope,
    name: *const c_char,
    out: *mut *mut AmScope,
) -> AmStatus {
    guard(|| {
        let out = unsafe { prepare_out(out) }?;
        let parent = unsafe { scope_ref(parent) }?;
        let name = unsafe { req_str(name, "name") }?;
        let handle = match &parent.kind {
            ScopeKind::Root => {
                let h = parent.shared.client()?.create_scope(name)?;
                parent.shared.track_scope(&h);
                h
            }
            ScopeKind::Child(s) => {
                parent.shared.check_alive()?;
                s.create_scope(name)?
            }
        };
        *out = Box::into_raw(Box::new(AmScope {
            kind: ScopeKind::Child(handle),
            shared: parent.shared.clone(),
        }));
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_scope_dispose(scope: *mut AmScope) -> AmStatus {
    guard(|| {
        let scope = unsafe { scope_ref(scope) }?;
        scope.shared.check_alive()?;
        match &scope.kind {
            ScopeKind::Root => scope.shared.dispose_root(),
            ScopeKind::Child(s) => s.dispose(),
        }
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_scope_free(scope: *mut AmScope) {
    if !scope.is_null() {
        // SAFETY: scope 来自本库，调用方保证不再使用。
        unsafe { consume(scope) };
    }
}

// ---------------------------------------------------------------------------
// 工具
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_tool_register(
    scope: *mut AmScope,
    spec: *const AmToolSpec,
    handler: Option<AmToolFn>,
    user_data: *mut c_void,
    free_user_data: Option<AmFreeFn>,
    out: *mut *mut AmTool,
) -> AmStatus {
    // SAFETY: 参数约定与 am_tool_register_ex 相同，options 为 NULL。
    unsafe { am_tool_register_ex(scope, spec, std::ptr::null(), handler, user_data, free_user_data, out) }
}

/// v9：同 `am_tool_register`，另带工具选项（`options` 可为 NULL）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_tool_register_ex(
    scope: *mut AmScope,
    spec: *const AmToolSpec,
    options: *const AmToolOptions,
    handler: Option<AmToolFn>,
    user_data: *mut c_void,
    free_user_data: Option<AmFreeFn>,
    out: *mut *mut AmTool,
) -> AmStatus {
    guard(|| {
        let ud = UserData::new(user_data, free_user_data);
        let out = unsafe { prepare_out(out) }?;
        let scope = unsafe { scope_ref(scope) }?;
        let spec = unsafe { spec.as_ref() }.ok_or_else(|| FfiError::null("spec"))?;
        let f = handler.ok_or_else(|| FfiError::null("handler"))?;
        let spec = unsafe { convert_tool_spec(spec, None) }?;
        // SAFETY: options 为 NULL 或指向至少 struct_size 字节。
        let options = unsafe { read_tool_options(options) }?;
        let handler = Arc::new(CToolHandler { f, user_data: ud });
        let handle = match &scope.kind {
            ScopeKind::Root => {
                let h = scope.shared.client()?.register_tool_with(spec, options, handler)?;
                scope.shared.track_tool(&h);
                h
            }
            ScopeKind::Child(s) => {
                scope.shared.check_alive()?;
                s.register_tool_with(spec, options, handler)?
            }
        };
        *out = Box::into_raw(Box::new(AmTool {
            handle,
            shared: scope.shared.clone(),
        }));
        Ok(())
    })
}

pub(crate) unsafe fn tool_ref<'a>(tool: *const AmTool) -> FfiResult<&'a AmTool> {
    let t = unsafe { tool.as_ref() }.ok_or_else(|| FfiError::null("tool"))?;
    t.shared.check_alive()?;
    Ok(t)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_tool_update(tool: *mut AmTool, spec: *const AmToolSpec) -> AmStatus {
    guard(|| {
        let tool = unsafe { tool_ref(tool) }?;
        let spec = unsafe { spec.as_ref() }.ok_or_else(|| FfiError::null("spec"))?;
        let spec = unsafe { convert_tool_spec(spec, Some(tool.handle.name())) }?;
        tool.handle.update(spec)?;
        Ok(())
    })
}

/// v9：用新定义与选项整体替换（`options` 为 NULL 或字段为 NULL 表示清除该声明）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_tool_update_ex(
    tool: *mut AmTool,
    spec: *const AmToolSpec,
    options: *const AmToolOptions,
) -> AmStatus {
    guard(|| {
        let tool = unsafe { tool_ref(tool) }?;
        let spec = unsafe { spec.as_ref() }.ok_or_else(|| FfiError::null("spec"))?;
        let spec = unsafe { convert_tool_spec(spec, Some(tool.handle.name())) }?;
        // SAFETY: options 为 NULL 或指向至少 struct_size 字节。
        let options = unsafe { read_tool_options(options) }?;
        tool.handle.update_with(spec, options)?;
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_tool_set_enabled(tool: *mut AmTool, enabled: bool) -> AmStatus {
    guard(|| {
        unsafe { tool_ref(tool) }?.handle.set_enabled(enabled)?;
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_tool_dispose(tool: *mut AmTool) -> AmStatus {
    guard(|| {
        unsafe { tool_ref(tool) }?.handle.dispose();
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_tool_free(tool: *mut AmTool) {
    if !tool.is_null() {
        unsafe { consume(tool) };
    }
}

// ---------------------------------------------------------------------------
// 资源
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_resource_register(
    scope: *mut AmScope,
    spec: *const AmResourceSpec,
    reader: Option<AmReadFn>,
    user_data: *mut c_void,
    free_user_data: Option<AmFreeFn>,
    out: *mut *mut AmResource,
) -> AmStatus {
    // SAFETY: 参数约定与 am_resource_register_ex 相同，options 为 NULL。
    unsafe { am_resource_register_ex(scope, spec, std::ptr::null(), reader, user_data, free_user_data, out) }
}

/// v8：同 `am_resource_register`，另带资源选项（`options` 可为 NULL）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_resource_register_ex(
    scope: *mut AmScope,
    spec: *const AmResourceSpec,
    options: *const AmResourceOptions,
    reader: Option<AmReadFn>,
    user_data: *mut c_void,
    free_user_data: Option<AmFreeFn>,
    out: *mut *mut AmResource,
) -> AmStatus {
    guard(|| {
        let ud = UserData::new(user_data, free_user_data);
        let out = unsafe { prepare_out(out) }?;
        let scope = unsafe { scope_ref(scope) }?;
        let spec = unsafe { spec.as_ref() }.ok_or_else(|| FfiError::null("spec"))?;
        let f = reader.ok_or_else(|| FfiError::null("reader"))?;
        let spec = unsafe { convert_resource_spec(spec) }?;
        // SAFETY: options 为 NULL 或指向至少 struct_size 字节。
        let options = unsafe { read_resource_options(options) }?;
        let reader = Arc::new(CResourceReader { f, user_data: ud });
        let handle = match &scope.kind {
            ScopeKind::Root => {
                let h = scope.shared.client()?.register_resource_with(spec, options, reader)?;
                scope.shared.track_resource(&h);
                h
            }
            ScopeKind::Child(s) => {
                scope.shared.check_alive()?;
                s.register_resource_with(spec, options, reader)?
            }
        };
        *out = Box::into_raw(Box::new(AmResource {
            handle,
            shared: scope.shared.clone(),
        }));
        Ok(())
    })
}

pub(crate) unsafe fn resource_ref<'a>(r: *const AmResource) -> FfiResult<&'a AmResource> {
    let r = unsafe { r.as_ref() }.ok_or_else(|| FfiError::null("resource"))?;
    r.shared.check_alive()?;
    Ok(r)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_resource_notify_changed(resource: *mut AmResource) -> AmStatus {
    guard(|| {
        unsafe { resource_ref(resource) }?.handle.notify_changed()?;
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_resource_dispose(resource: *mut AmResource) -> AmStatus {
    guard(|| {
        unsafe { resource_ref(resource) }?.handle.dispose();
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_resource_free(resource: *mut AmResource) {
    if !resource.is_null() {
        unsafe { consume(resource) };
    }
}
