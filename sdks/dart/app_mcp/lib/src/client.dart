// 惯用 Dart 封装：AppMcp 客户端、Scope、工具与资源句柄。
//
// # 线程模型
//
// C ABI 的所有回调（工具调用、资源读取、取消、状态、配对、free_user_data）都在库的
// 分发线程上执行。这里全部用 `NativeCallable.listener` 接收：库线程调用时只是把参数
// 投递到创建它的 isolate 的事件循环，随后在该 isolate（Flutter 中即主 isolate）上执行
// Dart 代码。因此 handler 总在创建 [AppMcp] 的 isolate 上运行，可以直接访问 UI 状态。
//
// 由此带来的约束：
// - listener 是异步的，Dart 代码执行时原生回调早已返回。C ABI 自 API 版本 2 起，状态回调的
//   reason、配对回调的 token、日志回调的 message 归接收方所有，回调返回后仍然有效；这里
//   读取后立即 `am_string_free`（无论目标对象是否还在，都要释放，否则泄漏）。
//   本文件按 AM_API_VERSION 2 编写，不能与 v1 的库混用（v1 下这些字符串会悬空）。
// - AmCall / AmRead 的所有权转移给回调方，在消费（complete / fail）前一直有效，可以异步使用。
//
// # NativeCallable 生命周期
//
// `free_user_data` 可能在库的任意线程、任意时间调用（最后一个引用被丢弃时），关闭后的
// NativeCallable 被调用是未定义行为。因此每个原生库实例只创建一组共享的 listener
// （工具、读取、取消、状态、配对、日志、free 各一个），设置 `keepIsolateAlive = false`，不关闭；
// user_data 只是整数 ID，指向 Dart 侧注册表中的对象。这样既不会调用已关闭的回调，
// 也不会因为回调而阻止 isolate 退出。注册表条目在库调用 free_user_data 或客户端 dispose 时删除。

import 'dart:async';
import 'dart:convert' show jsonEncode;
import 'dart:ffi';
import 'dart:io' show Platform;

import 'package:ffi/ffi.dart';

import 'bindings.dart';
import 'convert.dart';
import 'types.dart';

part 'client_runtime.dart';
part 'client_app.dart';
part 'client_scope.dart';
part 'client_handles.dart';
