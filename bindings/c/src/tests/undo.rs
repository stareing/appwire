//! v24 撤销（spec/protocol.md 3.8）：AmToolOptions.undoable 与 AmCallResult.undo_* 按 struct_size 读取、NULL 组合、
//! 错误码映射，以及 undoable 经核心进 toolsHash。结果中的撤销信息到达 Host 由一致性用例 result-undo（C 路径）覆盖。

use std::mem::offset_of;

use app_mcp_native::UndoAction;
use serde_json::json;

use super::*;
use super::cache::{cstr, offline_client, tool_options, tools_hash};
use crate::convert::undo_from;

fn p(s: Option<&CString>) -> *const c_char {
    s.map_or(ptr::null(), |s| s.as_ptr())
}

/// v24 全尺寸的调用结果（只给 data 与撤销信息）；指针借用调用方的 CString。
fn undo_result(data: &CString, tool: Option<&CString>, arguments: Option<&CString>, label: Option<&CString>) -> AmCallResult {
    AmCallResult {
        struct_size: std::mem::size_of::<AmCallResult>() as u32,
        data_json: data.as_ptr(),
        state_hints: ptr::null(),
        state_hints_len: 0,
        status: 0,
        state_resource: ptr::null(),
        summary: ptr::null(),
        annotations_json: ptr::null(),
        undo_tool: p(tool),
        undo_arguments_json: p(arguments),
        undo_label: p(label),
    }
}

fn action(tool: &str, arguments: serde_json::Value, label: Option<&str>) -> Option<UndoAction> {
    Some(UndoAction { tool: tool.into(), arguments, label: label.map(Into::into) })
}

/// 头文件的字段与上限说明与实现一致。
#[test]
fn header_declares_undo() {
    let header = include_str!("../../include/app_mcp.h");
    for field in ["bool undoable;", "const char *undo_tool;", "const char *undo_arguments_json;", "const char *undo_label;"] {
        assert!(header.contains(field), "{field}");
    }
    let limits = (app_mcp_native::MAX_UNDO_LABEL_CHARS, app_mcp_native::MAX_UNDO_ARGUMENTS_BYTES);
    assert!(header.contains(&format!("1..={} 个字符", limits.0)), "头文件的 label 上限与实现不一致");
    assert!(header.contains(&format!("不超过 {} 字节", limits.1)), "头文件的参数上限与实现不一致");
}

/// 64 位布局（C# / Dart 封装按此声明结构体）：v24 字段紧接 v23 末尾。
#[test]
#[cfg(target_pointer_width = "64")]
fn v24_layout() {
    use std::mem::size_of;
    assert_eq!(offset_of!(AmToolOptions, undoable), 120);
    assert_eq!(size_of::<AmToolOptions>(), 128);
    let offsets =
        (offset_of!(AmCallResult, undo_tool), offset_of!(AmCallResult, undo_arguments_json), offset_of!(AmCallResult, undo_label));
    assert_eq!(offsets, (64, 72, 80));
    assert_eq!(size_of::<AmCallResult>(), 88);
}

/// NULL 组合与错误码（T-09）：全 NULL = 不可撤销；参数 NULL = `{}`；格式问题原样交给核心；非法 JSON 可重试。
#[test]
fn undo_conversion() {
    let (tool, args, label) = (cstr("todo.remove"), cstr(r#"{"id":3}"#), cstr("删除刚添加的待办"));
    let read = |a: Option<&CString>, b: Option<&CString>, c: Option<&CString>| unsafe { undo_from(p(a), p(b), p(c)) };
    assert_eq!(read(None, None, None).ok(), Some(None));
    assert_eq!(read(Some(&tool), None, None).ok(), Some(Some(UndoAction::new("todo.remove"))));
    assert_eq!(read(Some(&tool), Some(&args), Some(&label)).ok(), Some(action("todo.remove", json!({"id": 3}), Some("删除刚添加的待办"))));
    // 不合法的格式不在 C 层判断：tool 为 NULL 时以空名交给核心，参数不是对象 / label 为空同样原样
    assert_eq!(read(None, None, Some(&label)).ok(), Some(action("", json!({}), Some("删除刚添加的待办"))));
    let (array, blank, bad_name) = (cstr("[1]"), cstr(""), cstr("bad name"));
    assert_eq!(read(Some(&tool), Some(&array), None).ok(), Some(action("todo.remove", json!([1]), None)));
    assert_eq!(read(Some(&bad_name), None, Some(&blank)).ok(), Some(action("bad name", json!({}), Some(""))));
    // 参数不是合法 JSON / UTF-8 → INVALID_JSON（可重试）；tool / label 非法 UTF-8 → INVALID_ARGUMENT
    let broken = cstr("{");
    assert_eq!(read(Some(&tool), Some(&broken), None).err().map(|e| e.status), Some(AmStatus::InvalidJson));
    let bad = [0xffu8, 0xfe, 0];
    let bad = bad.as_ptr().cast::<c_char>();
    assert_eq!(unsafe { undo_from(tool.as_ptr(), bad, ptr::null()) }.err().map(|e| e.status), Some(AmStatus::InvalidJson));
    for (a, c) in [(bad, ptr::null()), (tool.as_ptr(), bad)] {
        assert_eq!(unsafe { undo_from(a, ptr::null(), c) }.err().map(|e| e.status), Some(AmStatus::InvalidArgument));
    }
}

#[test]
fn call_result_undo_is_read_up_to_struct_size() {
    let (data, tool, args, label) = (cstr(r#"{"id":3}"#), cstr("todo.remove"), cstr(r#"{"id":3}"#), cstr("删除"));
    let full = undo_result(&data, Some(&tool), Some(&args), Some(&label));
    let read = |r: &AmCallResult| unsafe { read_call_result(r) }.ok().map(|r| r.undo);
    assert_eq!(read(&full), Some(action("todo.remove", json!({"id": 3}), Some("删除"))));
    // v23 调用方（到 annotations_json 为止）：不可撤销
    let v23 = AmCallResult { struct_size: offset_of!(AmCallResult, undo_tool) as u32, ..full };
    assert_eq!(read(&v23), Some(None));
    // 含 tool / arguments 但不含 label 的截断大小：同样按不可撤销处理（三字段同属 v24）
    let partial = AmCallResult { struct_size: offset_of!(AmCallResult, undo_label) as u32, ..full };
    assert_eq!(read(&partial), Some(None));
    assert_eq!(read(&undo_result(&data, None, None, None)), Some(None));
    let broken = cstr("{");
    let r = undo_result(&data, Some(&tool), Some(&broken), None);
    assert_eq!(unsafe { read_call_result(&r) }.err().map(|e| e.status), Some(AmStatus::InvalidJson));
}

#[test]
fn undoable_is_read_up_to_struct_size() {
    let full = AmToolOptions { undoable: true, ..tool_options(0, 0) };
    let read = |o: &AmToolOptions| unsafe { read_tool_options(o) }.ok().map(|o| o.undoable);
    assert_eq!(read(&full), Some(true));
    assert_eq!(read(&tool_options(0, 0)), Some(false));
    // v23 调用方（到 deprecated_until 为止）：按未声明处理
    let v23 = AmToolOptions { struct_size: offset_of!(AmToolOptions, undoable) as u32, ..full };
    assert_eq!(read(&v23), Some(false));
}

/// 经 C 接口注册 / 更新（不连接 Host）：undoable 进 toolsHash；false 与旧尺寸更新清除；am_tool_update 保留。
#[test]
fn undoable_register_and_update_through_c_abi() {
    let (client, root) = offline_client("c-abi-undo");
    let (tname, desc) = (cstr("todo.add"), cstr("添加待办"));
    let spec = AmToolSpec {
        name: tname.as_ptr(),
        description: desc.as_ptr(),
        input_schema_json: ptr::null(),
        risk: 1,
        activation: -1,
        title: ptr::null(),
        enabled: true,
    };
    let plain = tool_options(0, 0);
    let undoable = AmToolOptions { undoable: true, ..plain };
    let mut tool: *mut AmTool = ptr::null_mut();
    let rc = unsafe { am_tool_register_ex(root, &spec, &plain, Some(noop_tool), ptr::null_mut(), None, &mut tool) };
    assert_eq!(rc, AmStatus::Ok, "{}", last_error());
    let base = tools_hash(client);
    assert_eq!(unsafe { am_tool_update_ex(tool, &spec, &undoable) }, AmStatus::Ok);
    let declared = tools_hash(client);
    assert_ne!(declared, base, "undoable 进 toolsHash");
    assert_eq!(unsafe { am_tool_update(tool, &spec) }, AmStatus::Ok);
    assert_eq!(tools_hash(client), declared, "am_tool_update 保留已声明的选项");
    assert_eq!(unsafe { am_tool_update_ex(tool, &spec, &plain) }, AmStatus::Ok);
    assert_eq!(tools_hash(client), base, "false 清除声明");
    assert_eq!(unsafe { am_tool_update_ex(tool, &spec, &undoable) }, AmStatus::Ok);
    let v23 = AmToolOptions { struct_size: offset_of!(AmToolOptions, undoable) as u32, ..undoable };
    assert_eq!(unsafe { am_tool_update_ex(tool, &spec, &v23) }, AmStatus::Ok);
    assert_eq!(tools_hash(client), base, "旧尺寸更新 = 未声明");

    unsafe {
        am_tool_free(tool);
        am_scope_free(root);
        am_client_free(client);
    }
}
