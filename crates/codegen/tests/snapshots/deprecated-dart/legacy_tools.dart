// 由 app-mcp-codegen 生成（target: dart），请勿手动修改。
// App：弃用示例（legacy）
// 简介：演示工具级与参数级弃用的生成结果

import 'dart:async';

/// 按状态列出订单（旧版）
class OrdersListParams {
  const OrdersListParams({
    required this.status,
    this.page,
    this.limit,
  });

  factory OrdersListParams.fromJson(Map<String, dynamic> json) => OrdersListParams(
    status: json["status"] as String,
    page: (json["page"] as num?)?.toInt(),
    limit: (json["limit"] as num?)?.toInt(),
  );

  /// 订单状态
  @Deprecated("参数已弃用（inputSchema 中 deprecated: true）")
  final String status;

  /// 页码
  /// 取值范围：≥ 1
  @Deprecated("参数已弃用（inputSchema 中 deprecated: true）")
  final int? page;

  /// 最多返回条数
  final int? limit;

  Map<String, dynamic> toJson() => <String, dynamic>{
    "status": status,
    if (page != null) "page": page,
    if (limit != null) "limit": limit,
  };
}

/// 过滤条件
class OrdersFindFilter {
  const OrdersFindFilter({
    this.keyword,
    this.legacyTag,
  });

  factory OrdersFindFilter.fromJson(Map<String, dynamic> json) => OrdersFindFilter(
    keyword: json["keyword"] as String?,
    legacyTag: json["legacyTag"] as String?,
  );

  /// 关键词
  final String? keyword;

  /// 旧标签
  @Deprecated("参数已弃用（inputSchema 中 deprecated: true）")
  final String? legacyTag;

  Map<String, dynamic> toJson() => <String, dynamic>{
    if (keyword != null) "keyword": keyword,
    if (legacyTag != null) "legacyTag": legacyTag,
  };
}

/// 按状态查找订单
class OrdersFindParams {
  const OrdersFindParams({
    required this.state,
    this.cursor,
    this.filter,
  });

  factory OrdersFindParams.fromJson(Map<String, dynamic> json) => OrdersFindParams(
    state: json["state"] as String,
    cursor: json["cursor"] as String?,
    filter: json["filter"] == null ? null : OrdersFindFilter.fromJson(json["filter"] as Map<String, dynamic>),
  );

  /// 订单状态
  final String state;

  /// 分页游标
  final String? cursor;

  /// 过滤条件
  final OrdersFindFilter? filter;

  Map<String, dynamic> toJson() => <String, dynamic>{
    "state": state,
    if (cursor != null) "cursor": cursor,
    if (filter != null) "filter": filter!.toJson(),
  };
}

/// 清空购物车（旧版）
class CartLegacyClearParams {
  const CartLegacyClearParams();

  factory CartLegacyClearParams.fromJson(Map<String, dynamic> json) => const CartLegacyClearParams();

  Map<String, dynamic> toJson() => <String, dynamic>{};
}

/// 弃用示例 的工具实现。每个方法接收已由 Host 按 inputSchema 校验过的参数，
/// 返回值作为工具结果（可 JSON 编码的值）。
abstract interface class LegacyToolHandlers {
  /// 按状态列出订单（旧版）
  ///
  /// 工具 `orders.list`「列出订单」，风险：read
  @Deprecated("旧版 \"列表\" 接口 */ 不再维护：\$x \${y} \\(z) 'q' <b>&amp; #{w}\n请改用分页更好的新版（改用工具 orders.find；计划于 2027-06-30 移除）")
  FutureOr<Object?> ordersList(OrdersListParams params);

  /// 按状态查找订单
  ///
  /// 工具 `orders.find`「查找订单」，风险：read
  FutureOr<Object?> ordersFind(OrdersFindParams params);

  /// 清空购物车（旧版）
  ///
  /// 工具 `cart.legacyClear`「清空购物车」，风险：write
  @Deprecated("改用 cart.clear")
  FutureOr<Object?> cartLegacyClear(CartLegacyClearParams params);
}

/// 清单中的全部工具名。
const List<String> legacyToolNames = <String>[
  "orders.list",
  "orders.find",
  "cart.legacyClear",
];

/// 按工具名把调用分派到对应的 handler。参数应已由 Host 按 inputSchema 校验。
FutureOr<Object?> dispatchLegacyTool(LegacyToolHandlers handlers, String name, Map<String, dynamic>? arguments) {
  final args = arguments ?? const <String, dynamic>{};
  switch (name) {
    case "orders.list":
      return handlers.ordersList(OrdersListParams.fromJson(args));
    case "orders.find":
      return handlers.ordersFind(OrdersFindParams.fromJson(args));
    case "cart.legacyClear":
      return handlers.cartLegacyClear(CartLegacyClearParams.fromJson(args));
  }
  throw ArgumentError.value(name, 'name', '未知工具');
}
