// 由 app-mcp-codegen 生成（target: typescript），请勿手动修改。
// App：弃用示例（legacy）
// 简介：演示工具级与参数级弃用的生成结果

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
export type CartLegacyClearParams = Record<string, never>;

/**
 * 弃用示例 的工具实现。每个方法接收已由 Host 按 inputSchema 校验过的参数。
 * 返回值会序列化为 JSON 作为工具结果。
 */
export interface LegacyToolHandlers {
  /**
   * 按状态列出订单（旧版）
   *
   * 工具 `orders.list`「列出订单」，风险：read
   * @deprecated 旧版 "列表" 接口 *\/ 不再维护：$x ${y} \(z) 'q' <b>&amp; #{w}
   * 请改用分页更好的新版（改用工具 orders.find；计划于 2027-06-30 移除）
   */
  ordersList(params: OrdersListParams): unknown | Promise<unknown>;
  /**
   * 按状态查找订单
   *
   * 工具 `orders.find`「查找订单」，风险：read
   */
  ordersFind(params: OrdersFindParams): unknown | Promise<unknown>;
  /**
   * 清空购物车（旧版）
   *
   * 工具 `cart.legacyClear`「清空购物车」，风险：write
   * @deprecated 改用 cart.clear
   */
  cartLegacyClear(params: CartLegacyClearParams): unknown | Promise<unknown>;
}

/** 清单中的全部工具名。 */
export const legacyToolNames = [
  "orders.list",
  "orders.find",
  "cart.legacyClear",
] as const;

export type LegacyToolName = (typeof legacyToolNames)[number];

/** 工具名 → 参数类型。 */
export interface LegacyToolParams {
  "orders.list": OrdersListParams;
  "orders.find": OrdersFindParams;
  "cart.legacyClear": CartLegacyClearParams;
}

/**
 * 按工具名把调用分派到对应的 handler。
 * 参数应已由 Host 按 inputSchema 校验；这里只做类型断言。
 */
export function dispatchLegacyTool(handlers: LegacyToolHandlers, name: string, args: unknown): unknown | Promise<unknown> {
  const params: unknown = args ?? {};
  switch (name) {
    case "orders.list":
      return handlers.ordersList(params as OrdersListParams);
    case "orders.find":
      return handlers.ordersFind(params as OrdersFindParams);
    case "cart.legacyClear":
      return handlers.cartLegacyClear(params as CartLegacyClearParams);
    default:
      throw new Error(`未知工具：${name}`);
  }
}
