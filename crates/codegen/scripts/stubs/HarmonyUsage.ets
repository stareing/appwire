// verify.sh 的 harmony-insight-intents 步骤：用生成的 ShopTools / ShopInsightIntents 写一个最小接入，
// 检查 handler 接口、注册函数与 @app-mcp/harmony 的类型是否吻合。
import { AbilityStage } from '@kit.AbilityKit';
import { HarmonyAppMcp, ToolCallError, ToolResult } from '@app-mcp/harmony';
import {
  CartAddParams, CartCheckoutParams, CatalogSearchParams, ShopToolHandlers, TodosAddParams, registerShopTools,
} from './appmcp/ShopTools';
import { ShopIntentRuntime } from './appmcp/ShopInsightIntents';

class Item {
  id: string = '';
  qty: number = 0;
}

class Handlers implements ShopToolHandlers {
  catalogSearch(p: CatalogSearchParams): Object | null {
    return `${p.keyword ?? ''}:${p.category ?? 'all'}:${p.limit ?? 20}`;
  }

  async cartAdd(p: CartAddParams): Promise<Object | null> {
    const item = new Item();
    item.id = p.productId;
    item.qty = p.qty;
    return item;
  }

  cartRemoveItem(): Object | null {
    throw new ToolCallError('USER_REJECTED', '用户取消');
  }

  cartCheckout(p: CartCheckoutParams): Object | null {
    return new ToolResult<string>(p.shipping?.method ?? 'standard', ['cart']);
  }

  todosAdd(p: TodosAddParams): Object | null {
    return p.tags ?? [];
  }

  todosClear(): Object | null {
    return null;
  }

  ordersExport(): Object | null {
    return null;
  }

  statsSummary(): Object | null {
    return null;
  }
}

export default class ShopStage extends AbilityStage {
  onCreate(): void {
    const handlers = new Handlers();
    ShopIntentRuntime.handlers = handlers;
    const mcp = HarmonyAppMcp.create(this.context, { appId: 'shop', appName: '示例商城' }, 'shopapp');
    registerShopTools(mcp, handlers);
  }
}
