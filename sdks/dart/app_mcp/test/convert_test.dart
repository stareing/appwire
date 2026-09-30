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
      for (var i = 0; i <= AmStateStatus.waking; i++) {
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
      expect(durationToMs(const Duration(seconds: -1)), 0);
    });
    test('平台默认策略：移动端 idle + keep，iOS 隐藏即休眠', () {
      final ios = LifecyclePolicy.platformDefault(isAndroid: false, isIOS: true);
      expect(ios.mode, LifecycleMode.idle);
      expect(ios.residency, Residency.keep);
      expect(ios.hiddenIdleTimeout, Duration.zero);
      final android = LifecyclePolicy.platformDefault(isAndroid: true, isIOS: false);
      expect(android.mode, LifecycleMode.idle);
      expect(android.residency, Residency.keep);
      expect(android.hiddenIdleTimeout, const Duration(seconds: 15));
      expect(LifecyclePolicy.platformDefault(isAndroid: false, isIOS: false), const LifecyclePolicy());
      expect(const LifecyclePolicy().copyWith(mode: LifecycleMode.onDemand).mode, LifecycleMode.onDemand);
    });
  });
}
