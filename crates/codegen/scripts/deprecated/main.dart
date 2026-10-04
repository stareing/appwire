// verify.sh 的 deprecated 步骤：实现 deprecated.json 生成的 LegacyToolHandlers（含已弃用的方法）并检查 JSON 往返。
import 'dart:async';
import 'dart:convert';

import '../lib/legacy_tools.dart';

class Impl implements LegacyToolHandlers {
  @override
  FutureOr<Object?> ordersList(OrdersListParams params) => params.toJson();
  @override
  FutureOr<Object?> ordersFind(OrdersFindParams params) => params.toJson();
  @override
  FutureOr<Object?> cartLegacyClear(CartLegacyClearParams params) => 'cleared';
}

Future<void> main() async {
  final list = await dispatchLegacyTool(Impl(), 'orders.list', {'status': 'paid', 'page': 2});
  if (jsonEncode(list) != '{"status":"paid","page":2}') {
    throw StateError('orders.list：${jsonEncode(list)}');
  }
  final find = await dispatchLegacyTool(Impl(), 'orders.find', {'state': 's', 'filter': {'legacyTag': 't'}});
  if (jsonEncode(find) != '{"state":"s","filter":{"legacyTag":"t"}}') {
    throw StateError('orders.find：${jsonEncode(find)}');
  }
  print('ok');
}
