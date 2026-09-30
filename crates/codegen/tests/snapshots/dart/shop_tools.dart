// 由 app-mcp-codegen 生成（target: dart），请勿手动修改。
// App：示例商城（shop）
// 总览：演示用购物商城，可管理待办、浏览商品、操作购物车并结算

import 'dart:async';

/// 商品分类
enum CatalogSearchCategory {
  electronics("electronics"),
  homeGoods("home-goods"),
  food("food");

  const CatalogSearchCategory(this.value);

  /// JSON 中的取值。
  final String value;

  static CatalogSearchCategory fromJson(String value) => values.firstWhere(
    (e) => e.value == value,
    orElse: () => throw ArgumentError.value(value, 'value', '未知取值'),
  );

  String toJson() => value;
}

/// 按关键词搜索商品，返回商品 ID、名称、价格
class CatalogSearchParams {
  const CatalogSearchParams({
    this.keyword,
    this.category,
    this.limit,
    this.inStock,
    this.maxPrice,
  });

  factory CatalogSearchParams.fromJson(Map<String, dynamic> json) => CatalogSearchParams(
    keyword: json["keyword"] as String?,
    category: json["category"] == null ? null : CatalogSearchCategory.fromJson(json["category"] as String),
    limit: (json["limit"] as num?)?.toInt(),
    inStock: json["inStock"] as bool?,
    maxPrice: (json["maxPrice"] as num?)?.toDouble(),
  );

  /// 商品名或分类，如“耳机”
  final String? keyword;

  /// 商品分类
  final CatalogSearchCategory? category;

  /// 最多返回条数
  /// 取值范围：≥ 1，≤ 100；默认值：20
  final int? limit;

  /// 只看有货
  final bool? inStock;

  /// 价格上限（元）
  /// 取值范围：> 0
  final double? maxPrice;

  Map<String, dynamic> toJson() => <String, dynamic>{
    if (keyword != null) "keyword": keyword,
    if (category != null) "category": category!.value,
    if (limit != null) "limit": limit,
    if (inStock != null) "inStock": inStock,
    if (maxPrice != null) "maxPrice": maxPrice,
  };
}

/// 把商品加入购物车
class CartAddParams {
  const CartAddParams({
    required this.productId,
    required this.qty,
    this.note,
  });

  factory CartAddParams.fromJson(Map<String, dynamic> json) => CartAddParams(
    productId: json["productId"] as String,
    qty: (json["qty"] as num).toInt(),
    note: json["note"] as String?,
  );

  /// 商品 ID
  final String productId;

  /// 数量
  /// 取值范围：≥ 1
  final int qty;

  /// 备注，可为 null
  final String? note;

  Map<String, dynamic> toJson() => <String, dynamic>{
    "productId": productId,
    "qty": qty,
    if (note != null) "note": note,
  };
}

/// 从购物车移除一个条目
class CartRemoveItemParams {
  const CartRemoveItemParams({
    required this.itemId,
  });

  factory CartRemoveItemParams.fromJson(Map<String, dynamic> json) => CartRemoveItemParams(
    itemId: json["itemId"] as String,
  );

  final String itemId;

  Map<String, dynamic> toJson() => <String, dynamic>{
    "itemId": itemId,
  };
}

/// 配送方式
enum CartCheckoutShippingMethod {
  standard("standard"),
  express("express");

  const CartCheckoutShippingMethod(this.value);

  /// JSON 中的取值。
  final String value;

  static CartCheckoutShippingMethod fromJson(String value) => values.firstWhere(
    (e) => e.value == value,
    orElse: () => throw ArgumentError.value(value, 'value', '未知取值'),
  );

  String toJson() => value;
}

/// 配送选项
class CartCheckoutShipping {
  const CartCheckoutShipping({
    required this.method,
    this.deliverAfter,
  });

  factory CartCheckoutShipping.fromJson(Map<String, dynamic> json) => CartCheckoutShipping(
    method: CartCheckoutShippingMethod.fromJson(json["method"] as String),
    deliverAfter: json["deliverAfter"] as String?,
  );

  /// 配送方式
  final CartCheckoutShippingMethod method;

  /// 最早送达时间
  /// 格式：date-time
  final String? deliverAfter;

  Map<String, dynamic> toJson() => <String, dynamic>{
    "method": method.value,
    if (deliverAfter != null) "deliverAfter": deliverAfter,
  };
}

class CartCheckoutItemsItem {
  const CartCheckoutItemsItem({
    required this.itemId,
    this.qty,
  });

  factory CartCheckoutItemsItem.fromJson(Map<String, dynamic> json) => CartCheckoutItemsItem(
    itemId: json["itemId"] as String,
    qty: (json["qty"] as num?)?.toInt(),
  );

  final String itemId;

  /// 取值范围：≥ 1
  final int? qty;

  Map<String, dynamic> toJson() => <String, dynamic>{
    "itemId": itemId,
    if (qty != null) "qty": qty,
  };
}

/// 提交订单并支付
class CartCheckoutParams {
  const CartCheckoutParams({
    required this.addressId,
    this.coupon,
    this.shipping,
    this.items,
    this.giftWrap,
  });

  factory CartCheckoutParams.fromJson(Map<String, dynamic> json) => CartCheckoutParams(
    addressId: json["addressId"] as String,
    coupon: json["coupon"] as String?,
    shipping: json["shipping"] == null ? null : CartCheckoutShipping.fromJson(json["shipping"] as Map<String, dynamic>),
    items: (json["items"] as List<dynamic>?)?.map((e0) => CartCheckoutItemsItem.fromJson(e0 as Map<String, dynamic>)).toList(),
    giftWrap: json["giftWrap"] as bool?,
  );

  /// 收货地址 ID
  final String addressId;

  /// 优惠码
  final String? coupon;

  /// 配送选项
  final CartCheckoutShipping? shipping;

  /// 只结算这些条目；省略时结算全部
  /// 元素个数：≥ 1
  final List<CartCheckoutItemsItem>? items;

  /// 默认值：false
  final bool? giftWrap;

  Map<String, dynamic> toJson() => <String, dynamic>{
    "addressId": addressId,
    if (coupon != null) "coupon": coupon,
    if (shipping != null) "shipping": shipping!.toJson(),
    if (items != null) "items": items!.map((e0) => e0.toJson()).toList(),
    if (giftWrap != null) "giftWrap": giftWrap,
  };
}

/// 新增一条待办
class TodosAddParams {
  const TodosAddParams({
    required this.title,
    this.priority,
    this.tags,
    this.dueDate,
    this.class_,
    this.isUrgent,
  });

  factory TodosAddParams.fromJson(Map<String, dynamic> json) => TodosAddParams(
    title: json["title"] as String,
    priority: (json["priority"] as num?)?.toInt(),
    tags: (json["tags"] as List<dynamic>?)?.cast<String>(),
    dueDate: json["dueDate"] as String?,
    class_: json["class"] as String?,
    isUrgent: json["is-urgent"] as bool?,
  );

  /// 待办内容
  /// 长度：1–200
  final String title;

  /// 优先级
  /// 可选值：1, 2, 3
  final int? priority;

  /// 标签
  final List<String>? tags;

  /// 截止日期
  /// 格式：date
  final String? dueDate;

  /// 分类（属性名是保留字）
  final String? class_;

  /// 属性名含连字符
  final bool? isUrgent;

  Map<String, dynamic> toJson() => <String, dynamic>{
    "title": title,
    if (priority != null) "priority": priority,
    if (tags != null) "tags": tags,
    if (dueDate != null) "dueDate": dueDate,
    if (class_ != null) "class": class_,
    if (isUrgent != null) "is-urgent": isUrgent,
  };
}

/// 删除全部已完成的待办
class TodosClearParams {
  const TodosClearParams();

  factory TodosClearParams.fromJson(Map<String, dynamic> json) => const TodosClearParams();

  Map<String, dynamic> toJson() => <String, dynamic>{};
}

/// 按条件导出订单
class OrdersExportParams {
  const OrdersExportParams({
    this.filter,
    this.labels,
    this.extra,
    this.template,
  });

  factory OrdersExportParams.fromJson(Map<String, dynamic> json) => OrdersExportParams(
    filter: json["filter"],
    labels: (json["labels"] as Map<String, dynamic>?)?.map((k0, e0) => MapEntry(k0, e0 as String)),
    extra: json["extra"],
    template: json["template"],
  );

  /// 筛选条件（任意形式）
  /// 原始 JSON（不支持一般形式的 `oneOf`）
  final Object? filter;

  /// 附加标签
  final Map<String, String>? labels;

  /// 透传给导出器的任意 JSON
  final Object? extra;

  /// 原始 JSON（不支持 `$ref`）
  final Object? template;

  Map<String, dynamic> toJson() => <String, dynamic>{
    if (filter != null) "filter": filter,
    if (labels != null) "labels": labels,
    if (extra != null) "extra": extra,
    if (template != null) "template": template,
  };
}

/// 统计周期
enum StatsSummaryPeriod {
  day("day"),
  week("week"),
  month("month");

  const StatsSummaryPeriod(this.value);

  /// JSON 中的取值。
  final String value;

  static StatsSummaryPeriod fromJson(String value) => values.firstWhere(
    (e) => e.value == value,
    orElse: () => throw ArgumentError.value(value, 'value', '未知取值'),
  );

  String toJson() => value;
}

/// 查看销售概览
class StatsSummaryParams {
  const StatsSummaryParams({
    this.period,
  });

  factory StatsSummaryParams.fromJson(Map<String, dynamic> json) => StatsSummaryParams(
    period: json["period"] == null ? null : StatsSummaryPeriod.fromJson(json["period"] as String),
  );

  /// 统计周期
  final StatsSummaryPeriod? period;

  Map<String, dynamic> toJson() => <String, dynamic>{
    if (period != null) "period": period!.value,
  };
}

/// 示例商城 的工具实现。每个方法接收已由 Host 按 inputSchema 校验过的参数，
/// 返回值作为工具结果（可 JSON 编码的值）。
abstract interface class ShopToolHandlers {
  /// 按关键词搜索商品，返回商品 ID、名称、价格
  ///
  /// 工具 `catalog.search`「搜索商品」，风险：read
  FutureOr<Object?> catalogSearch(CatalogSearchParams params);

  /// 把商品加入购物车
  ///
  /// 工具 `cart.add`「加入购物车」，风险：write
  FutureOr<Object?> cartAdd(CartAddParams params);

  /// 从购物车移除一个条目
  ///
  /// 工具 `cart.removeItem`「移除购物车条目」，风险：destructive
  FutureOr<Object?> cartRemoveItem(CartRemoveItemParams params);

  /// 提交订单并支付
  ///
  /// 工具 `cart.checkout`「结算」，风险：payment
  FutureOr<Object?> cartCheckout(CartCheckoutParams params);

  /// 新增一条待办
  ///
  /// 工具 `todos.add`「新增待办」，风险：write
  FutureOr<Object?> todosAdd(TodosAddParams params);

  /// 删除全部已完成的待办
  ///
  /// 工具 `todos.clear`「清空待办」，风险：destructive
  FutureOr<Object?> todosClear(TodosClearParams params);

  /// 按条件导出订单
  ///
  /// 工具 `orders.export`「导出订单」，风险：write
  FutureOr<Object?> ordersExport(OrdersExportParams params);

  /// 查看销售概览
  ///
  /// 工具 `stats.summary`「销售概览」，风险：read
  FutureOr<Object?> statsSummary(StatsSummaryParams params);
}

/// 清单中的全部工具名。
const List<String> shopToolNames = <String>[
  "catalog.search",
  "cart.add",
  "cart.removeItem",
  "cart.checkout",
  "todos.add",
  "todos.clear",
  "orders.export",
  "stats.summary",
];

/// 按工具名把调用分派到对应的 handler。参数应已由 Host 按 inputSchema 校验。
FutureOr<Object?> dispatchShopTool(ShopToolHandlers handlers, String name, Map<String, dynamic>? arguments) {
  final args = arguments ?? const <String, dynamic>{};
  switch (name) {
    case "catalog.search":
      return handlers.catalogSearch(CatalogSearchParams.fromJson(args));
    case "cart.add":
      return handlers.cartAdd(CartAddParams.fromJson(args));
    case "cart.removeItem":
      return handlers.cartRemoveItem(CartRemoveItemParams.fromJson(args));
    case "cart.checkout":
      return handlers.cartCheckout(CartCheckoutParams.fromJson(args));
    case "todos.add":
      return handlers.todosAdd(TodosAddParams.fromJson(args));
    case "todos.clear":
      return handlers.todosClear(TodosClearParams.fromJson(args));
    case "orders.export":
      return handlers.ordersExport(OrdersExportParams.fromJson(args));
    case "stats.summary":
      return handlers.statsSummary(StatsSummaryParams.fromJson(args));
  }
  throw ArgumentError.value(name, 'name', '未知工具');
}
