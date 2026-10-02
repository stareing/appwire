// 兜底工具的执行引擎（spec/ui-fallback.md 第 2–7 节）：大纲、点击、填写、按键、滚动、读取。
// 与客户端、具体 UI 框架无关；对应 Android 的 UiInspector.kt。

import { ToolCallError } from '../Errors';
import { UiFallbackInput, UiFillValue, UiKey } from './UiFallbackInput';
import { UiActionResult, UiOutline, UiOutlineFormat, UiReadResult } from './UiOutlineFormat';
import { UiAction, UiElement, UiNode, UiRefRegistry, UiSnapshot, UiTreeCollector, UiWindow } from './UiTree';

/** 平台：当前可操作的窗口与"等界面稳定"（ArkUI 见 ArkUiPlatform；测试用假实现）。 */
export interface UiPlatform {
  /** 当前可见、未被模态层遮挡的窗口 / 对话框，按 Z 序（最上层在后）。 */
  windows(): UiWindow[];
  /** 全部窗口根（含被模态层遮挡的部分），用于区分"失效"与"不可见"。 */
  allWindows(): UiWindow[];
  /** 控件（identity）仍在界面上（可能不可见）。 */
  isLive(identity: string): boolean;
  /** 执行动作后等界面稳定（一帧 + 短暂延迟）。 */
  settle(): Promise<void>;
}

const TEXT_ROLES: string[] = ['textbox', 'searchbox'];
const STABLE_ROUNDS: number = 10;

export class UiInspector {
  readonly prefix: string;
  readonly maxItems: number;
  private readonly platform: UiPlatform;
  private readonly refs: UiRefRegistry = new UiRefRegistry();

  constructor(platform: UiPlatform, prefix: string = 'ui', maxItems: number = 60) {
    this.platform = platform;
    this.prefix = prefix;
    this.maxItems = Math.max(1, maxItems);
  }

  /** 丢弃全部引用（之后旧引用一律失效）。 */
  clear(): void {
    this.refs.clear();
  }

  private snapshot(): UiSnapshot {
    return UiTreeCollector.collect(this.refs, this.platform.windows(),
      (identity: string): boolean => this.platform.isLive(identity));
  }

  /** 按引用在当前界面中取控件（spec/ui-fallback.md 7.1）。 */
  private resolve(reference: string, snap: UiSnapshot): UiNode {
    const n = snap.byRef.get(reference);
    if (n !== undefined) {
      return n;
    }
    if (UiTreeCollector.findAttached(this.refs, reference, this.platform.allWindows()) !== undefined) {
      throw UiFallbackInput.hidden(`引用 ${reference} 对应的控件`, reference, this.prefix);
    }
    throw UiFallbackInput.stale(reference, this.prefix);
  }

  private actionable(reference: string, snap: UiSnapshot): UiNode {
    const n = this.resolve(reference, snap);
    if (!n.element.isEnabled()) {
      throw UiFallbackInput.disabled(n.label, reference);
    }
    return n;
  }

  private refuseSecure(n: UiNode): void {
    if (n.secure) {
      throw UiFallbackInput.secure(n.label, n.entry.ref);
    }
  }

  private unsupported(n: UiNode, action: UiAction, why?: string): ToolCallError {
    return UiFallbackInput.unsupported(n.label, n.entry.ref, why ?? n.element.unsupportedReason(action));
  }

  /** untilStable：带动画的动作再等到相邻两次快照没有变化（最多 STABLE_ROUNDS 轮）。 */
  private async act(before: UiSnapshot, target: UiNode | undefined, untilStable: boolean,
    run: () => void): Promise<UiActionResult> {
    run();
    await this.platform.settle();
    let after = this.snapshot();
    for (let round = 0; untilStable && round < STABLE_ROUNDS; round++) {
      await this.platform.settle();
      const next = this.snapshot();
      const moving = UiOutlineFormat.diff(after.entries, next.entries, this.prefix).length > 0;
      after = next;
      if (!moving) {
        break;
      }
    }
    const result: UiActionResult = { ok: true, changes: UiOutlineFormat.diff(before.entries, after.entries, this.prefix) };
    const declared = target?.entry.declared;
    if (declared !== undefined) {
      result.hint = `该元素已声明为工具 ${declared}，下次可直接调用`;
    }
    return result;
  }

  /** `ui.outline`（spec/ui-fallback.md 第 4 节）。 */
  outline(query: string | undefined, within: string | undefined, limit: number | undefined): UiOutline {
    const n = Math.min(UiOutlineFormat.LIMIT_MAX, Math.max(1, limit ?? this.maxItems));
    const snap = this.snapshot();
    if (within === undefined) {
      return UiOutlineFormat.render(snap.entries, query, n);
    }
    const scope = this.resolve(within, snap);
    const sub = UiTreeCollector.collect(this.refs, [], (identity: string): boolean => this.platform.isLive(identity), scope);
    return UiOutlineFormat.render(sub.entries, query, n);
  }

  /** `ui.click`：激活控件。 */
  async click(reference: string): Promise<UiActionResult> {
    const before = this.snapshot();
    const n = this.actionable(reference, before);
    return this.act(before, n, false, () => {
      if (!n.element.click()) {
        throw this.unsupported(n, 'click');
      }
    });
  }

  /** `ui.fill`：文本框写入文本；复选框 / 开关 / 单选框按布尔值切换；下拉框按选项文本；滑块按数字。 */
  async fill(reference: string, value: UiFillValue): Promise<UiActionResult> {
    const before = this.snapshot();
    const n = this.actionable(reference, before);
    this.refuseSecure(n);
    const e = n.entry;
    if (TEXT_ROLES.includes(e.role)) {
      const text = UiFallbackInput.text(value, n.label, reference);
      if (e.states.includes('readonly')) {
        throw this.unsupported(n, 'fill', '只读');
      }
      return this.act(before, n, false, () => {
        if (!n.element.setText(text)) {
          throw this.unsupported(n, 'fill');
        }
      });
    }
    if (e.role === 'checkbox' || e.role === 'switch' || e.role === 'radio') {
      const want = UiFallbackInput.bool(value, n.label, reference);
      if (e.states.includes('checked') === want) {
        return this.act(before, n, false, () => {});
      }
      if (e.role === 'radio' && !want) {
        throw this.unsupported(n, 'fill', '单选框不能直接取消选中，请选中同组的其他项');
      }
      return this.act(before, n, false, () => {
        if (!n.element.click()) {
          throw this.unsupported(n, 'click');
        }
      });
    }
    if (e.role === 'combobox') {
      const text = UiFallbackInput.text(value, n.label, reference);
      return this.act(before, n, false, () => {
        if (!n.element.selectOption(text)) {
          throw this.unsupported(n, 'fill');
        }
      });
    }
    if (e.role === 'slider' || e.role === 'spinbutton') {
      const num = UiFallbackInput.num(value, n.label, reference);
      return this.act(before, n, false, () => {
        if (!n.element.setRange(num)) {
          throw this.unsupported(n, 'fill');
        }
      });
    }
    throw this.unsupported(n, 'fill', '只能填写文本框、复选框、开关、单选框、下拉框与滑块');
  }

  /** `ui.press`（spec/ui-fallback.md 2.1）：reference 给出时先聚焦该控件。 */
  async press(reference: string | undefined, key: string): Promise<UiActionResult> {
    const k = UiFallbackInput.key(key);
    const before = this.snapshot();
    const target = reference !== undefined ? this.actionable(reference, before) : undefined;
    if (target !== undefined) {
      this.refuseSecure(target);
    }
    let focusedNode: UiNode | undefined = undefined;
    for (const n of before.byRef.values()) {
      if (n.entry.states.includes('focused')) {
        focusedNode = n;
      }
    }
    if (target === undefined && focusedNode !== undefined && focusedNode.secure) {
      throw UiFallbackInput.invalid('当前焦点在密码类控件上，兜底工具不对其按键', undefined, 'secure');
    }
    const windows = this.platform.windows();
    const subject: UiNode | undefined = target ?? focusedNode;
    const window: UiWindow | undefined = subject !== undefined ? subject.window : windows[windows.length - 1];
    if (window === undefined) {
      throw new ToolCallError('TOOL_DISABLED', '应用当前没有可见窗口');
    }
    let handled = true;
    const isText = subject !== undefined && TEXT_ROLES.includes(subject.entry.role);
    const activate = (): boolean => subject !== undefined && !isText && subject.element.click();
    const result = await this.act(before, target, false, () => {
      if (target !== undefined) {
        target.element.focus();
      }
      const el: UiElement | undefined = subject?.element;
      if (k === UiKey.TAB) {
        handled = window.moveFocus(true);
      } else if (k === UiKey.SHIFT_TAB) {
        handled = window.moveFocus(false);
      } else if (k === UiKey.ESCAPE) {
        const top: UiWindow | undefined = windows[windows.length - 1];
        handled = top !== undefined && (top.sendKey(k, undefined) || top.dismiss());
      } else if (k === UiKey.ENTER) {
        handled = (isText && el !== undefined && el.imeAction()) || window.sendKey(k, el) || activate();
      } else {
        handled = window.sendKey(k, el) || activate();
      }
    });
    if (!handled && result.hint === undefined) {
      result.hint = `按键 ${key} 没有被任何控件处理`;
    }
    return result;
  }

  /** `ui.scroll`：无 direction 时把控件滚动到可见，否则滚动控件（或其所在滚动区）一页。只核对引用仍在界面上。 */
  async scroll(reference: string, direction: string | undefined): Promise<UiActionResult> {
    const dir = UiFallbackInput.direction(direction);
    const before = this.snapshot();
    const visible = before.byRef.get(reference);
    const element: UiElement | undefined = visible !== undefined ? visible.element :
      UiTreeCollector.findAttached(this.refs, reference, this.platform.allWindows());
    if (element === undefined) {
      throw UiFallbackInput.stale(reference, this.prefix);
    }
    if (!element.canScroll()) {
      const label = visible !== undefined ? visible.label : `引用 ${reference} 对应的控件`;
      throw UiFallbackInput.unsupported(label, reference, element.unsupportedReason('scroll'));
    }
    let moved = false;
    if (dir === undefined) {
      if (visible !== undefined) {
        const none: UiActionResult = { ok: true, changes: [] };
        return none;
      }
      const r = await this.act(before, undefined, true, () => {
        moved = element.scrollIntoView();
      });
      if (!moved) {
        r.hint = '没有可滚动的祖先，无法滚动到可见';
      }
      return r;
    }
    const result = await this.act(before, undefined, true, () => {
      moved = element.scrollPage(dir);
    });
    if (!moved) {
      result.hint = '已到尽头或不可滚动';
    }
    return result;
  }

  /** `ui.read`：控件（缺省为全部窗口）的可见文本，折叠空白；密码类控件只返回掩码。 */
  read(reference: string | undefined, maxChars: number | undefined): UiReadResult {
    const max = Math.min(UiOutlineFormat.READ_MAX, Math.max(1, maxChars ?? UiOutlineFormat.READ_DEFAULT));
    const parts: string[] = [];
    const walk = (e: UiElement): void => {
      if (e.isHidden()) {
        return;
      }
      e.readText().forEach((t: string) => parts.push(t));
      if (e.readChildren()) {
        e.children().forEach((c: UiElement) => walk(c));
      }
    };
    if (reference !== undefined) {
      walk(this.resolve(reference, this.snapshot()).element);
    } else {
      this.platform.windows().forEach((w: UiWindow) => walk(w));
    }
    return UiOutlineFormat.readText(parts, reference, max);
  }
}
