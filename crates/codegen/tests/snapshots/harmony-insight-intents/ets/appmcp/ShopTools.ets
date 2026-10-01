// 由 app-mcp-codegen 生成（target: harmony-insight-intents），请勿手动修改。
// App：示例商城（shop）
// 总览：演示用购物商城，可管理待办、浏览商品、操作购物车并结算
//
// 注册为 MCP 工具：registerShopTools(appMcp, handlers)；意图执行器使用 ShopIntentRuntime.handlers（见 ShopInsightIntents.ets）。

import { Registrar, ToolHandle } from "@app-mcp/harmony";

/** 商品分类 */
export type CatalogSearchCategory = "electronics" | "home-goods" | "food";

/** 按关键词搜索商品，返回商品 ID、名称、价格 */
export interface CatalogSearchParams {
  /** 商品名或分类，如“耳机” */
  keyword?: string;
  /** 商品分类 */
  category?: CatalogSearchCategory;
  /**
   * 最多返回条数
   * 取值范围：≥ 1，≤ 100；默认值：20
   */
  limit?: number;
  /** 只看有货 */
  inStock?: boolean;
  /**
   * 价格上限（元）
   * 取值范围：> 0
   */
  maxPrice?: number;
}

/** 把商品加入购物车 */
export interface CartAddParams {
  /** 商品 ID */
  productId: string;
  /**
   * 数量
   * 取值范围：≥ 1
   */
  qty: number;
  /** 备注，可为 null */
  note?: string | null;
}

/** 从购物车移除一个条目 */
export interface CartRemoveItemParams {
  itemId: string;
}

/** 配送方式 */
export type CartCheckoutShippingMethod = "standard" | "express";

/** 配送选项 */
export interface CartCheckoutShipping {
  /** 配送方式 */
  method: CartCheckoutShippingMethod;
  /**
   * 最早送达时间
   * 格式：date-time
   */
  deliverAfter?: string;
}

export interface CartCheckoutItemsItem {
  itemId: string;
  /** 取值范围：≥ 1 */
  qty?: number;
}

/** 提交订单并支付 */
export interface CartCheckoutParams {
  /** 收货地址 ID */
  addressId: string;
  /** 优惠码 */
  coupon?: string;
  /** 配送选项 */
  shipping?: CartCheckoutShipping;
  /**
   * 只结算这些条目；省略时结算全部
   * 元素个数：≥ 1
   */
  items?: CartCheckoutItemsItem[];
  /** 默认值：false */
  giftWrap?: boolean;
}

/** 新增一条待办 */
export interface TodosAddParams {
  /**
   * 待办内容
   * 长度：1–200
   */
  title: string;
  /**
   * 优先级
   * 可选值：1, 2, 3
   */
  priority?: number;
  /** 标签 */
  tags?: string[];
  /**
   * 截止日期
   * 格式：date
   */
  dueDate?: string;
  /** 分类（属性名是保留字） */
  class?: string;
  /** 属性名含连字符 */
  "is-urgent"?: boolean;
}

/** 删除全部已完成的待办 */
export interface TodosClearParams {}

/** 按条件导出订单 */
export interface OrdersExportParams {
  /**
   * 筛选条件（任意形式）
   * 原始 JSON（不支持一般形式的 `oneOf`）
   */
  filter?: Object | null;
  /** 附加标签 */
  labels?: Record<string, string>;
  /** 透传给导出器的任意 JSON */
  extra?: Object | null;
  /** 原始 JSON（不支持 `$ref`） */
  template?: Object | null;
}

/** 统计周期 */
export type StatsSummaryPeriod = "day" | "week" | "month";

/** 查看销售概览 */
export interface StatsSummaryParams {
  /** 统计周期 */
  period?: StatsSummaryPeriod | null;
}

/**
 * 示例商城 的工具实现。MCP 调用时参数已由 Host 按 inputSchema 校验；
 * 意图调用时参数由系统入口按意图 parameters 赋值。返回值序列化为 JSON。
 */
export interface ShopToolHandlers {
  /**
   * 按关键词搜索商品，返回商品 ID、名称、价格
   *
   * 工具 `catalog.search`「搜索商品」，风险：read
   */
  catalogSearch(params: CatalogSearchParams): Object | null | Promise<Object | null>;
  /**
   * 把商品加入购物车
   *
   * 工具 `cart.add`「加入购物车」，风险：write
   */
  cartAdd(params: CartAddParams): Object | null | Promise<Object | null>;
  /**
   * 从购物车移除一个条目
   *
   * 工具 `cart.removeItem`「移除购物车条目」，风险：destructive
   */
  cartRemoveItem(params: CartRemoveItemParams): Object | null | Promise<Object | null>;
  /**
   * 提交订单并支付
   *
   * 工具 `cart.checkout`「结算」，风险：payment
   */
  cartCheckout(params: CartCheckoutParams): Object | null | Promise<Object | null>;
  /**
   * 新增一条待办
   *
   * 工具 `todos.add`「新增待办」，风险：write
   */
  todosAdd(params: TodosAddParams): Object | null | Promise<Object | null>;
  /**
   * 删除全部已完成的待办
   *
   * 工具 `todos.clear`「清空待办」，风险：destructive
   */
  todosClear(params: TodosClearParams): Object | null | Promise<Object | null>;
  /**
   * 按条件导出订单
   *
   * 工具 `orders.export`「导出订单」，风险：write
   */
  ordersExport(params: OrdersExportParams): Object | null | Promise<Object | null>;
  /**
   * 查看销售概览
   *
   * 工具 `stats.summary`「销售概览」，风险：read
   */
  statsSummary(params: StatsSummaryParams): Object | null | Promise<Object | null>;
}

/**
 * 把清单中的全部工具注册到 @app-mcp/harmony 的客户端或 scope，调用转给 handlers。
 * @error 重名或 schema 非法时抛出原生错误（code 为 DUPLICATE_NAME / INVALID_SCHEMA）。
 */
export function registerShopTools(registrar: Registrar, handlers: ShopToolHandlers): ToolHandle[] {
  return [
    registrar.tool<CatalogSearchParams, Object | null>("catalog.search", {
      description: "按关键词搜索商品，返回商品 ID、名称、价格",
      title: "搜索商品",
      inputSchema: "{\"properties\":{\"category\":{\"description\":\"商品分类\",\"enum\":[\"electronics\",\"home-goods\",\"food\"],\"type\":\"string\"},\"inStock\":{\"description\":\"只看有货\",\"type\":\"boolean\"},\"keyword\":{\"description\":\"商品名或分类，如“耳机”\",\"type\":\"string\"},\"limit\":{\"default\":20,\"description\":\"最多返回条数\",\"maximum\":100,\"minimum\":1,\"type\":\"integer\"},\"maxPrice\":{\"description\":\"价格上限（元）\",\"exclusiveMinimum\":0,\"type\":\"number\"}},\"type\":\"object\"}",
      risk: "read",
      activation: "headless",
      handler: (input: CatalogSearchParams): Object | null | Promise<Object | null> => handlers.catalogSearch(input),
    }),
    registrar.tool<CartAddParams, Object | null>("cart.add", {
      description: "把商品加入购物车",
      title: "加入购物车",
      inputSchema: "{\"properties\":{\"note\":{\"description\":\"备注，可为 null\",\"type\":[\"string\",\"null\"]},\"productId\":{\"description\":\"商品 ID\",\"type\":\"string\"},\"qty\":{\"description\":\"数量\",\"minimum\":1,\"type\":\"integer\"}},\"required\":[\"productId\",\"qty\"],\"type\":\"object\"}",
      risk: "write",
      activation: "background",
      handler: (input: CartAddParams): Object | null | Promise<Object | null> => handlers.cartAdd(input),
    }),
    registrar.tool<CartRemoveItemParams, Object | null>("cart.removeItem", {
      description: "从购物车移除一个条目",
      title: "移除购物车条目",
      inputSchema: "{\"properties\":{\"itemId\":{\"type\":\"string\"}},\"required\":[\"itemId\"],\"type\":\"object\"}",
      risk: "destructive",
      handler: (input: CartRemoveItemParams): Object | null | Promise<Object | null> => handlers.cartRemoveItem(input),
    }),
    registrar.tool<CartCheckoutParams, Object | null>("cart.checkout", {
      description: "提交订单并支付",
      title: "结算",
      inputSchema: "{\"properties\":{\"addressId\":{\"description\":\"收货地址 ID\",\"type\":\"string\"},\"coupon\":{\"description\":\"优惠码\",\"type\":\"string\"},\"giftWrap\":{\"default\":false,\"type\":\"boolean\"},\"items\":{\"description\":\"只结算这些条目；省略时结算全部\",\"items\":{\"properties\":{\"itemId\":{\"type\":\"string\"},\"qty\":{\"minimum\":1,\"type\":\"integer\"}},\"required\":[\"itemId\"],\"type\":\"object\"},\"minItems\":1,\"type\":\"array\"},\"shipping\":{\"description\":\"配送选项\",\"properties\":{\"deliverAfter\":{\"description\":\"最早送达时间\",\"format\":\"date-time\",\"type\":\"string\"},\"method\":{\"description\":\"配送方式\",\"enum\":[\"standard\",\"express\"]}},\"required\":[\"method\"],\"type\":\"object\"}},\"required\":[\"addressId\"],\"type\":\"object\"}",
      risk: "payment",
      activation: "foreground",
      handler: (input: CartCheckoutParams): Object | null | Promise<Object | null> => handlers.cartCheckout(input),
    }),
    registrar.tool<TodosAddParams, Object | null>("todos.add", {
      description: "新增一条待办",
      title: "新增待办",
      inputSchema: "{\"properties\":{\"class\":{\"description\":\"分类（属性名是保留字）\",\"type\":\"string\"},\"dueDate\":{\"description\":\"截止日期\",\"format\":\"date\",\"type\":\"string\"},\"is-urgent\":{\"description\":\"属性名含连字符\",\"type\":\"boolean\"},\"priority\":{\"description\":\"优先级\",\"enum\":[1,2,3],\"type\":\"integer\"},\"tags\":{\"description\":\"标签\",\"items\":{\"type\":\"string\"},\"type\":\"array\"},\"title\":{\"description\":\"待办内容\",\"maxLength\":200,\"minLength\":1,\"type\":\"string\"}},\"required\":[\"title\"],\"type\":\"object\"}",
      risk: "write",
      handler: (input: TodosAddParams): Object | null | Promise<Object | null> => handlers.todosAdd(input),
    }),
    registrar.tool<TodosClearParams, Object | null>("todos.clear", {
      description: "删除全部已完成的待办",
      title: "清空待办",
      inputSchema: "{\"properties\":{},\"type\":\"object\"}",
      risk: "destructive",
      handler: (input: TodosClearParams): Object | null | Promise<Object | null> => handlers.todosClear(input),
    }),
    registrar.tool<OrdersExportParams, Object | null>("orders.export", {
      description: "按条件导出订单",
      title: "导出订单",
      inputSchema: "{\"properties\":{\"extra\":{\"description\":\"透传给导出器的任意 JSON\"},\"filter\":{\"description\":\"筛选条件（任意形式）\",\"oneOf\":[{\"type\":\"string\"},{\"properties\":{\"status\":{\"type\":\"string\"}},\"type\":\"object\"}]},\"labels\":{\"additionalProperties\":{\"type\":\"string\"},\"description\":\"附加标签\",\"type\":\"object\"},\"template\":{\"$ref\":\"#/$defs/template\"}},\"type\":\"object\"}",
      risk: "write",
      handler: (input: OrdersExportParams): Object | null | Promise<Object | null> => handlers.ordersExport(input),
    }),
    registrar.tool<StatsSummaryParams, Object | null>("stats.summary", {
      description: "查看销售概览",
      title: "销售概览",
      inputSchema: "{\"properties\":{\"period\":{\"description\":\"统计周期\",\"enum\":[\"day\",\"week\",\"month\",null]}},\"type\":\"object\"}",
      risk: "read",
      handler: (input: StatsSummaryParams): Object | null | Promise<Object | null> => handlers.statsSummary(input),
    }),
  ];
}
