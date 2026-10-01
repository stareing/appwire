import 'dart:async';

import 'package:app_mcp/app_mcp.dart';
import 'package:app_mcp/src/bindings.dart';
import 'package:app_mcp/src/convert.dart';
import 'package:test/test.dart';

void main() {
  group('枚举映射', () {
    test('Risk 与头文件一致', () {
      expect(Risk.values.map(riskToNative), [0, 1, 2, 3, 4]);
      expect(Risk.osSensitive.wireName, 'os-sensitive');
    });

    test('Activation 缺省为 NONE(-1)', () {
      expect(activationToNative(null), AmActivation.none);
      expect(activationToNative(Activation.foreground), 2);
    });

    test('AppVisibility、ClientKind', () {
      expect(AppVisibility.values.map(visibilityToNative), [0, 1, 2]);
      expect(clientKindToNative(ClientKind.hybrid), 1);
    });

    test('状态码与状态', () {
      expect(statusFromNative(AmStateStatus.pendingPairing), ConnectionStatus.pendingPairing);
      expect(statusFromNative(AmStateStatus.dormant), ConnectionStatus.dormant);
      expect(statusFromNative(AmStateStatus.waking), ConnectionStatus.waking);
      expect(statusFromNative(AmStateStatus.hostMismatch), ConnectionStatus.hostMismatch);
      final mismatch = stateFromNative(AmStateStatus.hostMismatch, 0, '不是 app-mcp');
      expect(mismatch.reason, '不是 app-mcp');
      for (var i = 0; i <= AmStateStatus.hostMismatch; i++) {
        expect(statusFromNative(i).index, i, reason: '枚举顺序与 C 一致');
      }
      expect(statusFromNative(99), ConnectionStatus.idle);
      expect(errorCodeFromStatus(AmStatus.duplicateName), AppMcpErrorCode.duplicateName);
      expect(errorCodeFromStatus(AmStatus.panic), AppMcpErrorCode.panic);
      expect(errorCodeFromStatus(42), AppMcpErrorCode.unknown);
      expect(cancelReasonFromNative(AmCancelReason.disconnected), CancelReason.disconnected);
      expect(cancelReasonFromNative(99), CancelReason.requested);
    });

    test('stateFromNative 只在对应状态保留 retryIn / reason', () {
      expect(stateFromNative(AmStateStatus.backoff, 1500, null),
          const McpConnectionState(ConnectionStatus.backoff, retryIn: Duration(milliseconds: 1500)));
      expect(stateFromNative(AmStateStatus.connected, 1500, 'x'),
          const McpConnectionState(ConnectionStatus.connected));
      expect(stateFromNative(AmStateStatus.rejected, 0, 'bad token').reason, 'bad token');
      // v6：backoff 也保留原因（可为 null）。
      expect(stateFromNative(AmStateStatus.backoff, 0, '连接被拒绝').reason, '连接被拒绝');
    });

    test('stateFromNative 只在 backoff / rejected / hostMismatch 保留 code', () {
      for (final st in [AmStateStatus.backoff, AmStateStatus.rejected, AmStateStatus.hostMismatch]) {
        expect(stateFromNative(st, 0, 'r', code: 'HOST_NOT_RUNNING').code, 'HOST_NOT_RUNNING');
      }
      for (final st in [AmStateStatus.idle, AmStateStatus.connected, AmStateStatus.stopped, AmStateStatus.dormant]) {
        expect(stateFromNative(st, 0, null, code: 'X').code, isNull);
      }
      expect(stateFromNative(AmStateStatus.backoff, 0, null).code, isNull);
      const a = McpConnectionState(ConnectionStatus.backoff, code: 'A');
      expect(a == const McpConnectionState(ConnectionStatus.backoff, code: 'B'), isFalse);
      expect(a.hashCode == const McpConnectionState(ConnectionStatus.backoff, code: 'A').hashCode, isTrue);
      expect(a.toString(), contains('A'));
    });
  });

  group('ErrorKind', () {
    test('往返', () {
      for (final k in ErrorKind.values) {
        expect(ErrorKind.parse(k.wireName), k);
      }
    });
    test('未知值按 HANDLER_ERROR', () {
      expect(ErrorKind.parse('NOPE'), ErrorKind.handlerError);
    });
  });

  group('参数解码', () {
    test('对象', () {
      expect(decodeArguments('{"id":"a","qty":2}'), {'id': 'a', 'qty': 2});
    });
    test('空文本与 null 视为空对象', () {
      expect(decodeArguments(''), isEmpty);
      expect(decodeArguments('null'), isEmpty);
    });
    test('非对象为 INVALID_INPUT', () {
      expect(() => decodeArguments('[1]'),
          throwsA(isA<ToolCallError>().having((e) => e.kind, 'kind', ErrorKind.invalidInput)));
      expect(() => decodeArguments('{bad'),
          throwsA(isA<ToolCallError>().having((e) => e.kind, 'kind', ErrorKind.invalidInput)));
    });
  });

  group('结果编码', () {
    test('直接返回数据', () {
      final r = encodeResult({'ok': true});
      expect(r.dataJson, '{"ok":true}');
      expect(r.stateHints, isEmpty);
    });
    test('null', () {
      expect(encodeResult(null).dataJson, 'null');
    });
    test('ToolResult 带 stateHints', () {
      final r = encodeResult(const ToolResult([1, 2], stateHints: ['cart']));
      expect(r.dataJson, '[1,2]');
      expect(r.stateHints, ['cart']);
    });
    test('无法编码时为 HANDLER_ERROR', () {
      expect(() => encodeResult(Object()),
          throwsA(isA<ToolCallError>().having((e) => e.kind, 'kind', ErrorKind.handlerError)));
    });
  });

  group('错误映射', () {
    test('ToolCallError 保留类别', () {
      final f = failureFromError(ToolCallError(ErrorKind.toolDisabled, '购物车为空'));
      expect(f.kind, 'TOOL_DISABLED');
      expect(f.message, '购物车为空');
      expect(f.detailsJson, isNull);
    });
    test('ToolCallError.details 编码为 JSON；无法编码时为 null', () {
      expect(failureFromError(ToolCallError(ErrorKind.userRejected, 'x', details: {'a': 1})).detailsJson,
          '{"a":1}');
      expect(failureFromError(ToolCallError(ErrorKind.userRejected, 'x', details: [1, 2])).detailsJson,
          '[1,2]');
      expect(failureFromError(ToolCallError(ErrorKind.userRejected, 'x', details: Object())).detailsJson,
          isNull);
    });
    test('TimeoutException 为 TIMEOUT', () {
      expect(failureFromError(TimeoutException('慢')).kind, 'TIMEOUT');
    });
    test('其他异常为 HANDLER_ERROR', () {
      final f = failureFromError(StateError('boom'));
      expect(f.kind, 'HANDLER_ERROR');
      expect(f.message, contains('boom'));
    });
  });

  group('ToolSpec', () {
    test('相等性比较 schema 内容', () {
      const a = ToolSpec(name: 'a', description: 'd', inputSchema: {'type': 'object'});
      const b = ToolSpec(name: 'a', description: 'd', inputSchema: {'type': 'object'});
      expect(a, b);
      expect(a.hashCode, b.hashCode);
      expect(a == a.copyWith(risk: Risk.read), isFalse);
    });
    test('encodeSchema', () {
      expect(encodeSchema(null), isNull);
      expect(encodeSchema({'type': 'object'}), '{"type":"object"}');
    });
  });

  test('默认库名', () {
    expect(defaultNativeLibraryName(), anyOf('libapp_mcp.so', 'app_mcp.dll', 'libapp_mcp.dylib'));
  });

  group('生命周期转换', () {
    test('枚举映射与 C 头文件一致', () {
      expect(LifecycleMode.values.map(lifecycleModeToNative), [0, 1, 2]);
      expect(Residency.values.map(residencyToNative), [0, 1, 2]);
      expect(wakeKindToNative(null), -1);
      expect(WakeKind.values.map(wakeKindToNative), [0, 1, 2, 3, 4, 5, 6]);
      expect(WakeReason.values.map(wakeReasonToNative), [0, 1, 2, 3]);
      expect(SleepReason.values.map(sleepReasonToNative), [0, 1, 2, 3]);
      expect(statusFromNative(8), ConnectionStatus.dormant);
      expect(statusFromNative(9), ConnectionStatus.waking);
      expect(statusFromNative(10), ConnectionStatus.hostMismatch);
      expect(durationToMs(const Duration(seconds: -1)), 0);
    });
    test('平台默认策略：移动端 onDemand + 后台立即休眠 + keep，iOS 隐藏即休眠，桌面 persistent', () {
      final ios = LifecyclePolicy.platformDefault(isAndroid: false, isIOS: true);
      expect(ios.mode, LifecycleMode.onDemand);
      expect(ios.sleepOnBackground, isTrue);
      expect(ios.residency, Residency.keep);
      expect(ios.hiddenIdleTimeout, Duration.zero);
      final android = LifecyclePolicy.platformDefault(
          isAndroid: true, isIOS: false, wake: const WakeDescriptor.androidIntent('com.x/.WakeReceiver'));
      expect(android.mode, LifecycleMode.onDemand);
      expect(android.sleepOnBackground, isTrue);
      expect(android.residency, Residency.keep);
      expect(android.hiddenIdleTimeout, const Duration(seconds: 15));
      expect(android.wake, const WakeDescriptor.androidIntent('com.x/.WakeReceiver'));
      expect(android.mergeWindow, const Duration(seconds: 2));
      // 桌面没有单实例重定向（休眠后唤醒会冷启动新进程），即使给了 URI 唤醒描述也保持 persistent。
      expect(LifecyclePolicy.platformDefault(isAndroid: false, isIOS: false), const LifecyclePolicy());
      expect(
          LifecyclePolicy.platformDefault(isAndroid: false, isIOS: false, wake: const WakeDescriptor.uri('x')).mode,
          LifecycleMode.persistent);
      expect(const LifecyclePolicy().copyWith(mode: LifecycleMode.onDemand).mode, LifecycleMode.onDemand);
      // 覆盖优先：copyWith 只改指定字段。
      final custom = ios.copyWith(mode: LifecycleMode.idle, sleepOnBackground: false);
      expect(custom.mode, LifecycleMode.idle);
      expect(custom.sleepOnBackground, isFalse);
      expect(custom.hiddenIdleTimeout, Duration.zero);
    });

    test('功耗字段默认值、编码与相等性', () {
      const d = LifecyclePolicy();
      expect(d.hostAbsentRetries, 3);
      expect(d.legacyTimers, isFalse);
      expect(d.mergeWindow, const Duration(seconds: 2));
      expect(d.sleepOnBackground, isFalse);
      expect(HeartbeatMode.values.map(heartbeatToNative), [0, 1, 2]);
      expect(hostAbsentRetriesToNative(3), 3);
      expect(hostAbsentRetriesToNative(0), lessThan(0));
      expect(hostAbsentRetriesToNative(-5), lessThan(0));
      expect(hostAbsentRetriesToNative(1 << 40), 0x7FFFFFFF);
      expect(mergeWindowToNative(const Duration(seconds: 2)), 2000);
      expect(mergeWindowToNative(Duration.zero), lessThan(0));
      expect(mergeWindowToNative(const Duration(seconds: -1)), lessThan(0));
      final c = d.copyWith(hostAbsentRetries: 0, legacyTimers: true, mergeWindow: Duration.zero, sleepOnBackground: true);
      expect(c, isNot(d));
      expect(c, const LifecyclePolicy(
          hostAbsentRetries: 0, legacyTimers: true, mergeWindow: Duration.zero, sleepOnBackground: true));
      expect(c.hashCode, const LifecyclePolicy(
          hostAbsentRetries: 0, legacyTimers: true, mergeWindow: Duration.zero, sleepOnBackground: true).hashCode);
      expect(const ResourceSpec(name: 'a', description: 'b').realtime, isFalse);
    });
  });
}
