//! v23 工具弃用声明（spec/protocol.md 3.7）：按 struct_size 读取、NULL 组合、错误码映射，以及注册 / 更新经核心进 toolsHash。

use std::mem::offset_of;

use app_mcp_native::{Deprecation, MAX_DEPRECATION_MESSAGE_CHARS};

use super::*;
use super::cache::{cstr, noop_tool, offline_client, tool_options, tools_hash};
use crate::convert::deprecation_from;

/// 带弃用声明的 v23 全尺寸选项；指针借用调用方的 CString。
fn deprecated_options(message: Option<&CString>, replacement: Option<&CString>, until: Option<&CString>) -> AmToolOptions {
    let p = |s: Option<&CString>| s.map_or(ptr::null(), |s| s.as_ptr());
    AmToolOptions {
        deprecated_message: p(message),
        deprecated_replacement: p(replacement),
        deprecated_until: p(until),
        ..tool_options(0, 0)
    }
}

fn dep(message: &str, replacement: Option<&str>, until: Option<&str>) -> Option<Deprecation> {
    Some(Deprecation { message: message.into(), replacement: replacement.map(Into::into), until: until.map(Into::into) })
}

/// 头文件的上限说明与实现一致。
#[test]
fn header_declares_deprecation() {
    let header = include_str!("../../include/app_mcp.h");
    assert!(header.contains(&format!("1..={MAX_DEPRECATION_MESSAGE_CHARS} 个字符")), "头文件的 message 上限与实现不一致");
    for field in ["const char *deprecated_message;", "const char *deprecated_replacement;", "const char *deprecated_until;"] {
        assert!(header.contains(field), "{field}");
    }
}

/// 64 位布局（C# / Dart 封装按此声明结构体）：v23 字段紧接 v22 末尾（cache_scope 后 4 字节填充）。
#[test]
#[cfg(target_pointer_width = "64")]
fn v23_layout() {
    let offsets = (
        offset_of!(AmToolOptions, deprecated_message),
        offset_of!(AmToolOptions, deprecated_replacement),
        offset_of!(AmToolOptions, deprecated_until),
    );
    assert_eq!(offsets, (96, 104, 112));
    assert_eq!(offset_of!(AmToolOptions, undoable), 120, "v23 结束于 120（v24 字段由此开始）");
}

/// NULL 组合（T-09）：全 NULL = 未声明；只缺 message 时以空 message 交给核心；非法 UTF-8 → INVALID_ARGUMENT。
#[test]
fn deprecation_conversion() {
    let (m, r, u) = (cstr("改用 v2"), cstr("orders.list2"), cstr("2027-06-30"));
    let read = |a: Option<&CString>, b: Option<&CString>, c: Option<&CString>| {
        let p = |s: Option<&CString>| s.map_or(ptr::null(), |s| s.as_ptr());
        unsafe { deprecation_from(p(a), p(b), p(c)) }
    };
    assert_eq!(read(None, None, None).ok(), Some(None));
    assert_eq!(read(Some(&m), None, None).ok(), Some(dep("改用 v2", None, None)));
    assert_eq!(read(Some(&m), Some(&r), Some(&u)).ok(), Some(dep("改用 v2", Some("orders.list2"), Some("2027-06-30"))));
    assert_eq!(read(None, Some(&r), None).ok(), Some(dep("", Some("orders.list2"), None)));
    assert_eq!(read(None, None, Some(&u)).ok(), Some(dep("", None, Some("2027-06-30"))));
    let bad = [0xffu8, 0xfe, 0];
    let bad = bad.as_ptr().cast::<c_char>();
    for (a, b, c) in [(bad, ptr::null(), ptr::null()), (m.as_ptr(), bad, ptr::null()), (m.as_ptr(), ptr::null(), bad)] {
        assert_eq!(unsafe { deprecation_from(a, b, c) }.err().map(|e| e.status), Some(AmStatus::InvalidArgument));
    }
}

#[test]
fn deprecation_is_read_up_to_struct_size() {
    let (m, r, u) = (cstr("改用 v2"), cstr("orders.list2"), cstr("2027-06-30"));
    let full = deprecated_options(Some(&m), Some(&r), Some(&u));
    let read = |o: &AmToolOptions| unsafe { read_tool_options(o) }.ok().map(|o| o.deprecated);
    assert_eq!(read(&full), Some(dep("改用 v2", Some("orders.list2"), Some("2027-06-30"))));
    // v22 调用方（到 cache_scope 为止，含尾部填充）：按未声明处理
    let v22 = AmToolOptions { struct_size: offset_of!(AmToolOptions, deprecated_message) as u32, ..full };
    assert_eq!(read(&v22), Some(None));
    // 含 message / replacement 但不含 until 的截断大小：同样按未声明处理（三字段同属 v23）
    let partial = AmToolOptions { struct_size: offset_of!(AmToolOptions, deprecated_until) as u32, ..full };
    assert_eq!(read(&partial), Some(None));
    assert_eq!(read(&tool_options(0, 0)), Some(None));
}

/// 经 C 接口注册 / 更新（不连接 Host）：非法 → AM_ERR_INVALID_CONFIG 且保持原定义；声明进 toolsHash；全 NULL 清除。
#[test]
fn register_and_update_through_c_abi() {
    let (client, root) = offline_client("c-abi-deprecated");
    let (tname, desc) = (cstr("orders.list"), cstr("列出订单"));
    let spec = AmToolSpec {
        name: tname.as_ptr(),
        description: desc.as_ptr(),
        input_schema_json: ptr::null(),
        risk: 0,
        activation: -1,
        title: ptr::null(),
        enabled: true,
    };
    let (m, r, u) = (cstr("改用 orders.list2"), cstr("orders.list2"), cstr("2027-06-30"));
    let (self_ref, bad_date, long) = (cstr("orders.list"), cstr("2027-02-29"), cstr(&"字".repeat(MAX_DEPRECATION_MESSAGE_CHARS + 1)));
    let empty = cstr("  ");
    let invalid = [
        deprecated_options(Some(&empty), None, None),
        deprecated_options(Some(&long), None, None),
        deprecated_options(Some(&m), Some(&self_ref), None),
        deprecated_options(Some(&m), None, Some(&bad_date)),
        deprecated_options(None, Some(&r), None),
    ];
    let mut tool: *mut AmTool = ptr::null_mut();
    for options in &invalid {
        let rc = unsafe { am_tool_register_ex(root, &spec, options, Some(noop_tool), ptr::null_mut(), None, &mut tool) };
        assert_eq!(rc, AmStatus::InvalidConfig, "{}", last_error());
        assert!(tool.is_null());
        assert!(last_error().contains("deprecated"), "{}", last_error());
    }

    let plain = tool_options(0, 0);
    let rc = unsafe { am_tool_register_ex(root, &spec, &plain, Some(noop_tool), ptr::null_mut(), None, &mut tool) };
    assert_eq!(rc, AmStatus::Ok);
    let base = tools_hash(client);
    let full = deprecated_options(Some(&m), Some(&r), Some(&u));
    assert_eq!(unsafe { am_tool_update_ex(tool, &spec, &full) }, AmStatus::Ok);
    let with_full = tools_hash(client);
    assert_ne!(with_full, base, "deprecated 进 toolsHash");
    // 非法更新：失败且保持原定义
    for options in &invalid {
        assert_eq!(unsafe { am_tool_update_ex(tool, &spec, options) }, AmStatus::InvalidConfig);
    }
    assert_eq!(tools_hash(client), with_full);
    // am_tool_update 保留已声明的选项；替换为只有 message 的声明则摘要不同
    assert_eq!(unsafe { am_tool_update(tool, &spec) }, AmStatus::Ok);
    assert_eq!(tools_hash(client), with_full);
    let minimal = deprecated_options(Some(&m), None, None);
    assert_eq!(unsafe { am_tool_update_ex(tool, &spec, &minimal) }, AmStatus::Ok);
    let with_minimal = tools_hash(client);
    assert!(with_minimal != with_full && with_minimal != base);
    // 全 NULL 清除；v22 大小的选项也视为未声明（清除）
    assert_eq!(unsafe { am_tool_update_ex(tool, &spec, &plain) }, AmStatus::Ok);
    assert_eq!(tools_hash(client), base, "全 NULL 清除声明");
    assert_eq!(unsafe { am_tool_update_ex(tool, &spec, &full) }, AmStatus::Ok);
    let v22 = AmToolOptions { struct_size: offset_of!(AmToolOptions, deprecated_message) as u32, ..full };
    assert_eq!(unsafe { am_tool_update_ex(tool, &spec, &v22) }, AmStatus::Ok);
    assert_eq!(tools_hash(client), base, "旧尺寸更新 = 未声明");

    unsafe {
        am_tool_free(tool);
        am_scope_free(root);
        am_client_free(client);
    }
}
