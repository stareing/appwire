// 由 app-mcp-codegen 生成（target: typescript），请勿手动修改。
// App：示例商城（shop）
// 总览：演示用购物商城，可管理待办、浏览商品、操作购物车并结算

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
export type TodosClearParams = Record<string, never>;

/** 按条件导出订单 */
export interface OrdersExportParams {
  /**
   * 筛选条件（任意形式）
   * 原始 JSON（不支持一般形式的 `oneOf`）
   */
  filter?: unknown;
  /** 附加标签 */
  labels?: Record<string, string>;
  /** 透传给导出器的任意 JSON */
  extra?: unknown;
  /** 原始 JSON（不支持 `$ref`） */
  template?: unknown;
}

/** 统计周期 */
export type StatsSummaryPeriod = "day" | "week" | "month";

/** 查看销售概览 */
export interface StatsSummaryParams {
  /** 统计周期 */
  period?: StatsSummaryPeriod | null;
}

/**
 * 示例商城 的工具实现。每个方法接收已由 Host 按 inputSchema 校验过的参数。
 * 返回值会序列化为 JSON 作为工具结果。
 */
export interface ShopToolHandlers {
  /**
   * 按关键词搜索商品，返回商品 ID、名称、价格
   *
   * 工具 `catalog.search`「搜索商品」，风险：read
   */
  catalogSearch(params: CatalogSearchParams): unknown | Promise<unknown>;
  /**
   * 把商品加入购物车
   *
   * 工具 `cart.add`「加入购物车」，风险：write
   */
  cartAdd(params: CartAddParams): unknown | Promise<unknown>;
  /**
   * 从购物车移除一个条目
   *
   * 工具 `cart.removeItem`「移除购物车条目」，风险：destructive
   */
  cartRemoveItem(params: CartRemoveItemParams): unknown | Promise<unknown>;
  /**
   * 提交订单并支付
   *
   * 工具 `cart.checkout`「结算」，风险：payment
   */
  cartCheckout(params: CartCheckoutParams): unknown | Promise<unknown>;
  /**
   * 新增一条待办
   *
   * 工具 `todos.add`「新增待办」，风险：write
   */
  todosAdd(params: TodosAddParams): unknown | Promise<unknown>;
  /**
   * 删除全部已完成的待办
   *
   * 工具 `todos.clear`「清空待办」，风险：destructive
   */
  todosClear(params: TodosClearParams): unknown | Promise<unknown>;
  /**
   * 按条件导出订单
   *
   * 工具 `orders.export`「导出订单」，风险：write
   */
  ordersExport(params: OrdersExportParams): unknown | Promise<unknown>;
  /**
   * 查看销售概览
   *
   * 工具 `stats.summary`「销售概览」，风险：read
   */
  statsSummary(params: StatsSummaryParams): unknown | Promise<unknown>;
}

/** 清单中的全部工具名。 */
export const shopToolNames = [
  "catalog.search",
  "cart.add",
  "cart.removeItem",
  "cart.checkout",
  "todos.add",
  "todos.clear",
  "orders.export",
  "stats.summary",
] as const;

export type ShopToolName = (typeof shopToolNames)[number];

/** 工具名 → 参数类型。 */
export interface ShopToolParams {
  "catalog.search": CatalogSearchParams;
  "cart.add": CartAddParams;
  "cart.removeItem": CartRemoveItemParams;
  "cart.checkout": CartCheckoutParams;
  "todos.add": TodosAddParams;
  "todos.clear": TodosClearParams;
  "orders.export": OrdersExportParams;
  "stats.summary": StatsSummaryParams;
}

/**
 * 按工具名把调用分派到对应的 handler。
 * 参数应已由 Host 按 inputSchema 校验；这里只做类型断言。
 */
export function dispatchShopTool(handlers: ShopToolHandlers, name: string, args: unknown): unknown | Promise<unknown> {
  const params: unknown = args ?? {};
  switch (name) {
    case "catalog.search":
      return handlers.catalogSearch(params as CatalogSearchParams);
    case "cart.add":
      return handlers.cartAdd(params as CartAddParams);
    case "cart.removeItem":
      return handlers.cartRemoveItem(params as CartRemoveItemParams);
    case "cart.checkout":
      return handlers.cartCheckout(params as CartCheckoutParams);
    case "todos.add":
      return handlers.todosAdd(params as TodosAddParams);
    case "todos.clear":
      return handlers.todosClear(params as TodosClearParams);
    case "orders.export":
      return handlers.ordersExport(params as OrdersExportParams);
    case "stats.summary":
      return handlers.statsSummary(params as StatsSummaryParams);
    default:
      throw new Error(`未知工具：${name}`);
  }
}
