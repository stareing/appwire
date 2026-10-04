// 由 app-mcp-codegen 生成（target: harmony-insight-intents），请勿手动修改。
// App：弃用示例（legacy）
// 简介：演示工具级与参数级弃用的生成结果
//
// 注册为 MCP 工具：registerLegacyTools(appMcp, handlers)；意图执行器使用 LegacyIntentRuntime.handlers（见 LegacyInsightIntents.ets）。

import { Registrar, ToolHandle } from "@app-mcp/harmony";

/** 按状态列出订单（旧版） */
export interface OrdersListParams {
  /**
   * 订单状态
   * @deprecated 参数已弃用（inputSchema 中 deprecated: true）
   */
  status: string;
  /**
   * 页码
   * 取值范围：≥ 1
   * @deprecated 参数已弃用（inputSchema 中 deprecated: true）
   */
  page?: number;
  /** 最多返回条数 */
  limit?: number;
}

/** 过滤条件 */
export interface OrdersFindFilter {
  /** 关键词 */
  keyword?: string;
  /**
   * 旧标签
   * @deprecated 参数已弃用（inputSchema 中 deprecated: true）
   */
  legacyTag?: string;
}

/** 按状态查找订单 */
export interface OrdersFindParams {
  /** 订单状态 */
  state: string;
  /** 分页游标 */
  cursor?: string;
  /** 过滤条件 */
  filter?: OrdersFindFilter;
}

/** 清空购物车（旧版） */
export interface CartLegacyClearParams {}

/**
 * 弃用示例 的工具实现。MCP 调用时参数已由 Host 按 inputSchema 校验；
 * 意图调用时参数由系统入口按意图 parameters 赋值。返回值序列化为 JSON。
 */
export interface LegacyToolHandlers {
  /**
   * 按状态列出订单（旧版）
   *
   * 工具 `orders.list`「列出订单」，风险：read
   * @deprecated 旧版 "列表" 接口 *\/ 不再维护：$x ${y} \(z) 'q' <b>&amp; #{w}
   * 请改用分页更好的新版（改用工具 orders.find；计划于 2027-06-30 移除）
   */
  ordersList(params: OrdersListParams): Object | null | Promise<Object | null>;
  /**
   * 按状态查找订单
   *
   * 工具 `orders.find`「查找订单」，风险：read
   */
  ordersFind(params: OrdersFindParams): Object | null | Promise<Object | null>;
  /**
   * 清空购物车（旧版）
   *
   * 工具 `cart.legacyClear`「清空购物车」，风险：write
   * @deprecated 改用 cart.clear
   */
  cartLegacyClear(params: CartLegacyClearParams): Object | null | Promise<Object | null>;
}

/**
 * 把清单中的全部工具注册到 @app-mcp/harmony 的客户端或 scope，调用转给 handlers。
 * @error 重名或 schema 非法时抛出原生错误（code 为 DUPLICATE_NAME / INVALID_SCHEMA）。
 */
export function registerLegacyTools(registrar: Registrar, handlers: LegacyToolHandlers): ToolHandle[] {
  return [
    registrar.tool<OrdersListParams, Object | null>("orders.list", {
      description: "按状态列出订单（旧版）",
      title: "列出订单",
      inputSchema: "{\"properties\":{\"limit\":{\"description\":\"最多返回条数\",\"type\":\"integer\"},\"page\":{\"deprecated\":true,\"description\":\"页码\",\"minimum\":1,\"type\":\"integer\"},\"status\":{\"deprecated\":true,\"description\":\"订单状态\",\"type\":\"string\"}},\"required\":[\"status\"],\"type\":\"object\"}",
      risk: "read",
      activation: "headless",
      handler: (input: OrdersListParams): Object | null | Promise<Object | null> => handlers.ordersList(input),
    }),
    registrar.tool<OrdersFindParams, Object | null>("orders.find", {
      description: "按状态查找订单",
      title: "查找订单",
      inputSchema: "{\"properties\":{\"cursor\":{\"description\":\"分页游标\",\"type\":\"string\"},\"filter\":{\"description\":\"过滤条件\",\"properties\":{\"keyword\":{\"description\":\"关键词\",\"type\":\"string\"},\"legacyTag\":{\"deprecated\":true,\"description\":\"旧标签\",\"type\":\"string\"}},\"type\":\"object\"},\"state\":{\"description\":\"订单状态\",\"type\":\"string\"}},\"required\":[\"state\"],\"type\":\"object\"}",
      risk: "read",
      activation: "headless",
      handler: (input: OrdersFindParams): Object | null | Promise<Object | null> => handlers.ordersFind(input),
    }),
    registrar.tool<CartLegacyClearParams, Object | null>("cart.legacyClear", {
      description: "清空购物车（旧版）",
      title: "清空购物车",
      inputSchema: "{\"type\":\"object\"}",
      risk: "write",
      activation: "background",
      handler: (input: CartLegacyClearParams): Object | null | Promise<Object | null> => handlers.cartLegacyClear(input),
    }),
  ];
}
