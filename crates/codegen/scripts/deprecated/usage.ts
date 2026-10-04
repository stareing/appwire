// verify.sh 的 deprecated 步骤：实现 deprecated.json 生成的 LegacyToolHandlers（含已弃用的方法）并分派。
import { dispatchLegacyTool, type LegacyToolHandlers } from "./legacyTools.js";

const handlers: LegacyToolHandlers = {
  ordersList: (p) => [p.status, p.page ?? 1, p.limit ?? 20],
  ordersFind: (p) => [p.state, p.filter?.keyword ?? ""],
  cartLegacyClear: () => null,
};
export const result = dispatchLegacyTool(handlers, "orders.list", { status: "paid" });
