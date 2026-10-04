// verify.sh 的 deprecated 步骤：实现 deprecated.json 生成的 LegacyToolHandlers（含已弃用的方法）并注册。
import { AbilityStage } from '@kit.AbilityKit';
import { HarmonyAppMcp } from '@app-mcp/harmony';
import { LegacyIntentRuntime } from './appmcp/LegacyInsightIntents';
import { LegacyToolHandlers, OrdersFindParams, OrdersListParams, registerLegacyTools } from './appmcp/LegacyTools';

class Handlers implements LegacyToolHandlers {
  ordersList(p: OrdersListParams): Object | null {
    return `${p.status}:${p.page ?? 1}`;
  }

  ordersFind(p: OrdersFindParams): Object | null {
    return p.state;
  }

  cartLegacyClear(): Object | null {
    return null;
  }
}

export default class LegacyStage extends AbilityStage {
  onCreate(): void {
    const handlers = new Handlers();
    LegacyIntentRuntime.handlers = handlers;
    const mcp = HarmonyAppMcp.create(this.context, { appId: 'legacy', appName: '弃用示例' }, 'legacyapp');
    registerLegacyTools(mcp, handlers);
  }
}
