//! v22 结果缓存声明（spec/protocol.md 3.6）：按 struct_size 读取、取值校验、错误码映射，以及注册 / 更新经核心进 toolsHash。

use std::mem::offset_of;

use app_mcp_native::{CachePolicy, CacheScope, MAX_CACHE_TTL_MS};

use super::*;
use crate::convert::cache_policy_from;

pub(super) fn cstr(s: &str) -> CString {
    CString::new(s).unwrap_or_default()
}

pub(super) fn tool_options(ttl_ms: u64, scope: i32) -> AmToolOptions {
    AmToolOptions {
        struct_size: std::mem::size_of::<AmToolOptions>() as u32,
        annotations_json: ptr::null(),
        output_schema_json: ptr::null(),
        page: ptr::null(),
        surface: 0,
        background_tool: ptr::null(),
        concurrency: 0,
        exclusive: ptr::null(),
        implements: ptr::null(),
        implements_len: 0,
        cache_ttl_ms: ttl_ms,
        cache_scope: scope,
        deprecated_message: ptr::null(),
        deprecated_replacement: ptr::null(),
        deprecated_until: ptr::null(),
    }
}

fn resource_options(ttl_ms: u64, scope: i32) -> AmResourceOptions {
    AmResourceOptions {
        struct_size: std::mem::size_of::<AmResourceOptions>() as u32,
        realtime: false,
        annotations_json: ptr::null(),
        cache_ttl_ms: ttl_ms,
        cache_scope: scope,
    }
}

fn policy(ttl_ms: u64, scope: CacheScope) -> Option<CachePolicy> {
    Some(CachePolicy { ttl_ms, scope })
}

/// 头文件的枚举值、上限说明与实现一致。
#[test]
fn header_declares_cache_scope() {
    let header = include_str!("../../include/app_mcp.h");
    assert!(header.contains("AM_CACHE_PRIVATE = 0") && cache_policy_from(1, 0).ok() == Some(policy(1, CacheScope::Private)));
    assert!(header.contains("AM_CACHE_SHARED = 1") && cache_policy_from(1, 1).ok() == Some(policy(1, CacheScope::Shared)));
    assert!(header.contains(&format!("1..={MAX_CACHE_TTL_MS}")), "头文件的 ttl 上限与 MAX_CACHE_TTL_MS 不一致");
}

/// 64 位布局（C# / Dart 封装按此声明结构体）：v22 字段紧接 v21 末尾。
#[test]
#[cfg(target_pointer_width = "64")]
fn v22_layout() {
    use std::mem::size_of;
    assert_eq!((offset_of!(AmToolOptions, cache_ttl_ms), offset_of!(AmToolOptions, cache_scope)), (80, 88));
    assert_eq!((offset_of!(AmResourceOptions, cache_ttl_ms), offset_of!(AmResourceOptions, cache_scope)), (16, 24));
    assert_eq!(size_of::<AmResourceOptions>(), 32);
}

#[test]
fn cache_policy_conversion() {
    assert_eq!(cache_policy_from(0, 0).ok(), Some(None), "0 = 未声明");
    assert_eq!(cache_policy_from(0, 1).ok(), Some(None), "0 时 scope 被忽略");
    assert_eq!(cache_policy_from(5000, 0).ok(), Some(policy(5000, CacheScope::Private)));
    // 越界的 ttl 原样交给核心校验（InvalidConfig），这里不重复判断
    assert_eq!(cache_policy_from(MAX_CACHE_TTL_MS + 1, 1).ok(), Some(policy(MAX_CACHE_TTL_MS + 1, CacheScope::Shared)));
    for bad in [2, -1, 7] {
        assert_eq!(cache_policy_from(5000, bad).err().map(|e| e.status), Some(AmStatus::InvalidArgument), "{bad}");
        assert_eq!(cache_policy_from(0, bad).err().map(|e| e.status), Some(AmStatus::InvalidArgument), "{bad}");
    }
}

#[test]
fn tool_cache_is_read_up_to_struct_size() {
    let full = tool_options(60_000, 1);
    let read = unsafe { read_tool_options(&full) }.ok().map(|o| o.cache);
    assert_eq!(read, Some(policy(60_000, CacheScope::Shared)));
    // v21 调用方（到 implements_len 为止）：按未声明处理
    let v21 = AmToolOptions { struct_size: offset_of!(AmToolOptions, cache_ttl_ms) as u32, ..full };
    assert_eq!(unsafe { read_tool_options(&v21) }.ok().map(|o| o.cache), Some(None));
    // 含 ttl 但不含 scope 的截断大小：同样按未声明处理（两字段同属 v22）
    let partial = AmToolOptions { struct_size: offset_of!(AmToolOptions, cache_scope) as u32, ..full };
    assert_eq!(unsafe { read_tool_options(&partial) }.ok().map(|o| o.cache), Some(None));
    let zero = tool_options(0, 0);
    assert_eq!(unsafe { read_tool_options(&zero) }.ok().map(|o| o.cache), Some(None));
    let bad_scope = tool_options(1000, 9);
    assert_eq!(unsafe { read_tool_options(&bad_scope) }.err().map(|e| e.status), Some(AmStatus::InvalidArgument));
}

#[test]
fn resource_cache_is_read_up_to_struct_size() {
    let full = resource_options(30_000, 1);
    let read = unsafe { read_resource_options(&full) }.ok().map(|o| o.cache);
    assert_eq!(read, Some(policy(30_000, CacheScope::Shared)));
    let private = resource_options(5, 0);
    assert_eq!(unsafe { read_resource_options(&private) }.ok().map(|o| o.cache), Some(policy(5, CacheScope::Private)));
    // v13–v21 调用方（到 annotations_json 为止）：按未声明处理
    let v13 = AmResourceOptions { struct_size: offset_of!(AmResourceOptions, cache_ttl_ms) as u32, ..full };
    assert_eq!(unsafe { read_resource_options(&v13) }.ok().map(|o| o.cache), Some(None));
    let bad_scope = resource_options(1000, -3);
    assert_eq!(unsafe { read_resource_options(&bad_scope) }.err().map(|e| e.status), Some(AmStatus::InvalidArgument));
}

pub(super) fn tools_hash(client: *const AmClient) -> String {
    let p = unsafe { am_client_tools_hash(client) };
    let s = unsafe { opt_str(p, "hash") }.ok().flatten().unwrap_or_default().to_owned();
    unsafe { am_string_free(p) };
    s
}

/// 不连接 Host 的客户端（不 start）及其根作用域；由调用方释放。
pub(super) fn offline_client(app_id: &str) -> (*mut AmClient, *mut AmScope) {
    let (id, name, url) = (cstr(app_id), cstr("C ABI Test"), cstr("ws://127.0.0.1:1"));
    let cfg = AmClientConfig {
        app_id: id.as_ptr(),
        app_name: name.as_ptr(),
        instance_id: ptr::null(),
        host_url: url.as_ptr(),
        app_version: ptr::null(),
        instance_title: ptr::null(),
        token: ptr::null(),
        launch_token: ptr::null(),
        client_kind: 0,
        max_concurrent_calls: 0,
        overview_summary: ptr::null(),
        overview_body: ptr::null(),
        overview_locale: ptr::null(),
    };
    let mut client: *mut AmClient = ptr::null_mut();
    assert_eq!(unsafe { am_client_new(&cfg, ptr::null(), &mut client) }, AmStatus::Ok);
    let mut root: *mut AmScope = ptr::null_mut();
    assert_eq!(unsafe { am_client_root_scope(client, &mut root) }, AmStatus::Ok);
    (client, root)
}

/// 经 C 接口注册 / 更新（不连接 Host）：越界 → AM_ERR_INVALID_CONFIG 且保持原定义；声明进 toolsHash；ttl 0 清除。
#[test]
fn register_and_update_through_c_abi() {
    let (client, root) = offline_client("c-abi-cache");
    let desc = cstr("查询报价");
    let tname = cstr("stock.quote");
    let spec = AmToolSpec {
        name: tname.as_ptr(),
        description: desc.as_ptr(),
        input_schema_json: ptr::null(),
        risk: 0,
        activation: -1,
        title: ptr::null(),
        enabled: true,
    };
    let mut tool: *mut AmTool = ptr::null_mut();
    let too_long = tool_options(MAX_CACHE_TTL_MS + 1, 0);
    let rc = unsafe { am_tool_register_ex(root, &spec, &too_long, Some(noop_tool), ptr::null_mut(), None, &mut tool) };
    assert_eq!(rc, AmStatus::InvalidConfig);
    assert!(tool.is_null());
    assert!(last_error().contains("ttlMs"), "{}", last_error());

    let plain = tool_options(0, 0);
    let rc = unsafe { am_tool_register_ex(root, &spec, &plain, Some(noop_tool), ptr::null_mut(), None, &mut tool) };
    assert_eq!(rc, AmStatus::Ok);
    let base = tools_hash(client);
    let cached = tool_options(MAX_CACHE_TTL_MS, 1);
    assert_eq!(unsafe { am_tool_update_ex(tool, &spec, &cached) }, AmStatus::Ok);
    let with_cache = tools_hash(client);
    assert_ne!(with_cache, base, "cache 进 toolsHash");
    // 越界与非法 scope：更新失败，工具保持原定义
    assert_eq!(unsafe { am_tool_update_ex(tool, &spec, &too_long) }, AmStatus::InvalidConfig);
    assert_eq!(unsafe { am_tool_update_ex(tool, &spec, &tool_options(1000, 5)) }, AmStatus::InvalidArgument);
    assert_eq!(tools_hash(client), with_cache);
    // am_tool_update 保留已声明的选项；scope 不同则摘要不同
    assert_eq!(unsafe { am_tool_update(tool, &spec) }, AmStatus::Ok);
    assert_eq!(tools_hash(client), with_cache);
    assert_eq!(unsafe { am_tool_update_ex(tool, &spec, &tool_options(MAX_CACHE_TTL_MS, 0)) }, AmStatus::Ok);
    assert_ne!(tools_hash(client), with_cache);
    // ttl 0 清除；v21 大小的选项也视为未声明（清除）
    assert_eq!(unsafe { am_tool_update_ex(tool, &spec, &cached) }, AmStatus::Ok);
    assert_eq!(unsafe { am_tool_update_ex(tool, &spec, &plain) }, AmStatus::Ok);
    assert_eq!(tools_hash(client), base, "ttl 0 清除声明");
    assert_eq!(unsafe { am_tool_update_ex(tool, &spec, &cached) }, AmStatus::Ok);
    let v21 = AmToolOptions { struct_size: offset_of!(AmToolOptions, cache_ttl_ms) as u32, ..cached };
    assert_eq!(unsafe { am_tool_update_ex(tool, &spec, &v21) }, AmStatus::Ok);
    assert_eq!(tools_hash(client), base, "旧尺寸更新 = 未声明");

    // 资源：越界 → INVALID_CONFIG；声明进 toolsHash（与同名未声明的注册比较）
    let rname = cstr("quotes");
    let rspec = AmResourceSpec { name: rname.as_ptr(), description: desc.as_ptr(), mime_type: ptr::null() };
    let register = |options: &AmResourceOptions| {
        let mut res: *mut AmResource = ptr::null_mut();
        let rc = unsafe { am_resource_register_ex(root, &rspec, options, Some(noop_reader), ptr::null_mut(), None, &mut res) };
        (rc, res)
    };
    let (rc, res) = register(&resource_options(MAX_CACHE_TTL_MS + 1, 0));
    assert_eq!(rc, AmStatus::InvalidConfig);
    assert!(res.is_null());
    let (rc, res) = register(&resource_options(0, 0));
    assert_eq!(rc, AmStatus::Ok);
    let without = tools_hash(client);
    unsafe {
        assert_eq!(am_resource_dispose(res), AmStatus::Ok);
        am_resource_free(res);
    }
    let (rc, res) = register(&resource_options(30_000, 1));
    assert_eq!(rc, AmStatus::Ok);
    assert_ne!(tools_hash(client), without, "资源 cache 进 toolsHash");

    unsafe {
        am_resource_free(res);
        am_tool_free(tool);
        am_scope_free(root);
        am_client_free(client);
    }
}

pub(super) unsafe extern "C" fn noop_tool(_: *mut c_void, call: *mut AmCall) {
    unsafe { am_call_complete(call, ptr::null(), ptr::null(), 0) };
}

unsafe extern "C" fn noop_reader(_: *mut c_void, read: *mut AmRead) {
    unsafe { am_read_complete(read, ptr::null()) };
}
