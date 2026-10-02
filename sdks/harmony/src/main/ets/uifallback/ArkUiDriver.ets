// ArkUiWindowDriver 的缺省实现：经 UIContext 读取检查器树、按 id / uniqueId 派发动作。
// 所用接口（OpenHarmony SDK API 20 d.ts）：
//   UIContext.getFilteredInspectorTree / getFrameNodeByUniqueId / getFocusController / dispatchKeyEvent
//     —— ets/api/@ohos.arkui.UIContext.d.ts（API 12；dispatchKeyEvent API 15）
//   FrameNode.getCustomProperty（API 12）/ getInteractionEventBindingInfo（API 19）—— ets/api/arkui/FrameNode.d.ts
//   sendEventByKey(id, 10, '') —— ets/api/@internal/full/global.d.ts（标注 @test，见 README「控件兜底」的未知项）
//   KeyCode —— ets/api/@ohos.multimodalInput.keyCode.d.ts；IntentionCode —— @ohos.multimodalInput.intentionCode.d.ts

import { FrameNode, UIContext } from '@kit.ArkUI';
import { IntentionCode, KeyCode } from '@kit.InputKit';
import { ArkUiWindowDriver } from './ArkUiElements';
import { UiKey } from './UiFallbackInput';

/** sendEventByKey 的点击事件类型（global.d.ts：Click event: 10）。 */
const CLICK_EVENT: number = 10;

function keyCodeOf(key: UiKey): number {
  switch (key) {
    case UiKey.ENTER:
      return KeyCode.KEYCODE_ENTER;
    case UiKey.ESCAPE:
      return KeyCode.KEYCODE_ESCAPE;
    case UiKey.SPACE:
      return KeyCode.KEYCODE_SPACE;
    default:
      return KeyCode.KEYCODE_TAB;
  }
}

/** 一个窗口（主窗口或子窗口）的 UIContext 驱动。只在 UI 线程使用。 */
export class UIContextDriver implements ArkUiWindowDriver {
  readonly title: string;
  private readonly context: UIContext;

  constructor(context: UIContext, title: string) {
    this.context = context;
    this.title = title;
  }

  inspectorTree(): string {
    try {
      return this.context.getFilteredInspectorTree();
    } catch (error) {
      throw new Error(`getFilteredInspectorTree 失败：${(error as Error).message}`);
    }
  }

  private node(uniqueId: number): FrameNode | null {
    try {
      return this.context.getFrameNodeByUniqueId(uniqueId);
    } catch (error) {
      return null;
    }
  }

  customProperty(uniqueId: number, name: string): string | undefined {
    const value: Object | undefined = this.node(uniqueId)?.getCustomProperty(name);
    return typeof value === 'string' ? value as string : undefined;
  }

  hasClickHandler(uniqueId: number): boolean {
    const node = this.node(uniqueId);
    if (node === null) {
      return false;
    }
    try {
      return node.getInteractionEventBindingInfo(EventQueryType.ON_CLICK) !== undefined;
    } catch (error) {
      // @compat API 19 以下没有该接口：不把绑定了 onClick 的普通组件列为按钮。
      return false;
    }
  }

  clickById(inspectorId: string): boolean {
    return sendEventByKey(inspectorId, CLICK_EVENT, '');
  }

  requestFocus(inspectorId: string): boolean {
    try {
      this.context.getFocusController().requestFocus(inspectorId);
      return true;
    } catch (error) {
      return false;
    }
  }

  dispatchKey(uniqueId: number, key: UiKey): boolean {
    const code = keyCodeOf(key);
    const event = (type: KeyType): KeyEvent => {
      const e: KeyEvent = {
        type: type,
        keyCode: code,
        keyText: key,
        keySource: KeySource.Keyboard,
        deviceId: 0,
        metaKey: 0,
        timestamp: Date.now() * 1000000,
        stopPropagation: (): void => {},
        intentionCode: IntentionCode.INTENTION_UNKNOWN,
      };
      return e;
    };
    try {
      const down = this.context.dispatchKeyEvent(uniqueId, event(KeyType.Down));
      const up = this.context.dispatchKeyEvent(uniqueId, event(KeyType.Up));
      return down || up;
    } catch (error) {
      return false;
    }
  }
}
