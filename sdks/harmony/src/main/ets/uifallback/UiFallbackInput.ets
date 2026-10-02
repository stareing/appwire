// 兜底工具的参数解析、输入 schema 与错误（spec/ui-fallback.md 第 2 节、7.1）；对应 Android 的 UiFallbackInput.kt。

import { ToolCallError } from '../Errors';
import { UiOutlineFormat } from './UiOutlineFormat';

/** 支持的按键（spec/ui-fallback.md 2.1）。 */
export enum UiKey {
  ENTER = 'Enter',
  ESCAPE = 'Escape',
  TAB = 'Tab',
  SHIFT_TAB = 'Shift+Tab',
  SPACE = 'Space',
}

/** 查看方向（`down` = 向下翻看更多内容）。 */
export type UiScrollDirection = 'up' | 'down' | 'left' | 'right';

/** `ui.fill` 的值：文本、数字或布尔。 */
export type UiFillValue = string | number | boolean;

/** 工具参数（`JSON.parse` 的结果）。 */
export type UiArgs = Record<string, Object>;

const KEYS: Record<string, UiKey> = {
  'enter': UiKey.ENTER, 'return': UiKey.ENTER, 'escape': UiKey.ESCAPE, 'esc': UiKey.ESCAPE,
  'tab': UiKey.TAB, 'shift+tab': UiKey.SHIFT_TAB, ' ': UiKey.SPACE, 'space': UiKey.SPACE,
};

const DIRECTIONS: string[] = ['up', 'down', 'left', 'right'];

export class UiFallbackInput {
  static invalid(message: string, reference?: string, reason?: string): ToolCallError {
    const details: Record<string, Object> = {};
    let empty = true;
    if (reference !== undefined) {
      details['ref'] = reference;
      empty = false;
    }
    if (reason !== undefined) {
      details['reason'] = reason;
      empty = false;
    }
    return new ToolCallError('INVALID_INPUT', message, empty ? undefined : details);
  }

  static stale(reference: string, prefix: string): ToolCallError {
    return UiFallbackInput.invalid(`引用 ${reference} 已失效，请重新调用 ${prefix}.outline`, reference);
  }

  static hidden(label: string, reference: string, prefix: string): ToolCallError {
    return UiFallbackInput.invalid(`${label}当前不可见，请重新调用 ${prefix}.outline`, reference, 'hidden');
  }

  static disabled(label: string, reference: string): ToolCallError {
    return UiFallbackInput.invalid(`${label}已禁用，当前无法操作`, reference, 'TOOL_DISABLED');
  }

  static secure(label: string, reference?: string): ToolCallError {
    return UiFallbackInput.invalid(`${label}是密码类控件，兜底工具不填写`, reference, 'secure');
  }

  static unsupported(label: string, reference?: string, why?: string): ToolCallError {
    return UiFallbackInput.invalid(`${label}不支持该操作` + (why !== undefined ? `：${why}` : ''), reference, 'unsupported');
  }

  private static get(args: UiArgs, key: string): Object | undefined {
    const v: Object | undefined | null = args[key];
    return v === null ? undefined : v;
  }

  static str(args: UiArgs, key: string): string | undefined {
    const v = UiFallbackInput.get(args, key);
    if (v === undefined) {
      return undefined;
    }
    if (typeof v !== 'string') {
      throw UiFallbackInput.invalid(`${key} 应为字符串`);
    }
    return v as string;
  }

  static int(args: UiArgs, key: string): number | undefined {
    const v = UiFallbackInput.get(args, key);
    if (v === undefined) {
      return undefined;
    }
    if (typeof v !== 'number' || !Number.isFinite(v as number)) {
      throw UiFallbackInput.invalid(`${key} 应为正整数`);
    }
    return Math.floor(v as number);
  }

  static ref(args: UiArgs, key: string, required: boolean): string | undefined {
    const v = UiFallbackInput.get(args, key);
    if (v === undefined || v === '') {
      if (required) {
        throw UiFallbackInput.invalid(`缺少参数 ${key}（控件引用，如 "e12"）`);
      }
      return undefined;
    }
    const s = typeof v === 'string' ? (v as string).trim() : '';
    if (!UiOutlineFormat.REF_PATTERN.test(s)) {
      throw UiFallbackInput.invalid(`${key} 应为控件引用，如 "e12"`);
    }
    return s;
  }

  static requiredRef(args: UiArgs, key: string): string {
    const r = UiFallbackInput.ref(args, key, true);
    return r ?? '';
  }

  static value(args: UiArgs): UiFillValue {
    const v = UiFallbackInput.get(args, 'value');
    if (v === undefined) {
      throw UiFallbackInput.invalid('缺少参数 value');
    }
    if (typeof v === 'string') {
      return v as string;
    }
    if (typeof v === 'boolean') {
      return v as boolean;
    }
    if (typeof v === 'number' && Number.isFinite(v as number)) {
      return v as number;
    }
    throw UiFallbackInput.invalid('value 应为文本、数字或布尔');
  }

  static key(key: string): UiKey {
    const k = KEYS[key === ' ' ? key : key.trim().toLowerCase()];
    if (k === undefined) {
      throw UiFallbackInput.invalid(`不支持的按键「${key}」；支持 Enter、Escape、Tab、Shift+Tab、Space`);
    }
    return k;
  }

  static direction(direction: string | undefined): UiScrollDirection | undefined {
    if (direction === undefined) {
      return undefined;
    }
    if (!DIRECTIONS.includes(direction)) {
      throw UiFallbackInput.invalid('direction 应为 up / down / left / right');
    }
    return direction as UiScrollDirection;
  }

  /** 文本值：字符串原样，数字转十进制文本。 */
  static text(value: UiFillValue, label: string, reference?: string): string {
    if (typeof value === 'boolean') {
      throw UiFallbackInput.invalid(`${label}需要文本值`, reference);
    }
    return String(value);
  }

  static bool(value: UiFillValue, label: string, reference?: string): boolean {
    if (typeof value !== 'boolean') {
      throw UiFallbackInput.invalid(`${label}需要 true / false`, reference);
    }
    return value as boolean;
  }

  static num(value: UiFillValue, label: string, reference?: string): number {
    if (typeof value === 'number') {
      return value as number;
    }
    if (typeof value === 'string') {
      const n = Number((value as string).trim());
      if ((value as string).trim() !== '' && Number.isFinite(n)) {
        return n;
      }
    }
    throw UiFallbackInput.invalid(`${label}需要数字`, reference);
  }
}

/** 输入 schema（spec/ui-fallback.md 第 2 节），JSON 文本。 */
export class UiFallbackSchemas {
  private static refProp(description: string): string {
    return `{"type":"string","description":${JSON.stringify(description)}}`;
  }

  private static readonly REF: string = UiFallbackSchemas.refProp('控件引用，如 "e12"（来自 outline）');

  static outline(maxItems: number): string {
    return '{"type":"object","properties":{' +
      '"query":{"type":"string","description":"按名称模糊过滤（空格分隔多个词，全部匹配）"},' +
      `"within":${UiFallbackSchemas.refProp('只列出该引用分组的子树，如 "e40"')},` +
      `"limit":{"type":"integer","minimum":1,"maximum":${UiOutlineFormat.LIMIT_MAX},` +
      `"description":"最多列出的控件数，默认 ${maxItems}"}},"additionalProperties":false}`;
  }

  static readonly REF_ONLY: string =
    `{"type":"object","properties":{"ref":${UiFallbackSchemas.REF}},"required":["ref"],"additionalProperties":false}`;

  static readonly FILL: string = `{"type":"object","properties":{"ref":${UiFallbackSchemas.REF},"value":{` +
    '"description":"文本、数字（滑块）、布尔（复选框 / 开关 / 单选框）或选项文本（下拉框）",' +
    '"anyOf":[{"type":"string"},{"type":"number"},{"type":"boolean"}]}},' +
    '"required":["ref","value"],"additionalProperties":false}';

  static readonly PRESS: string =
    `{"type":"object","properties":{"ref":${UiFallbackSchemas.refProp('目标控件引用；缺省为当前焦点控件')},` +
    '"key":{"type":"string","description":"按键，如 \\"Enter\\"、\\"Escape\\"、\\"Tab\\"、\\"Shift+Tab\\"、\\"Space\\""}},' +
    '"required":["key"],"additionalProperties":false}';

  static readonly SCROLL: string = `{"type":"object","properties":{"ref":${UiFallbackSchemas.REF},` +
    '"direction":{"type":"string","enum":["up","down","left","right"],"description":"查看方向；缺省为滚动到该控件可见"}},' +
    '"required":["ref"],"additionalProperties":false}';

  static readonly READ: string =
    `{"type":"object","properties":{"ref":${UiFallbackSchemas.refProp('控件引用；缺省为全部窗口')},` +
    `"maxChars":{"type":"integer","minimum":1,"maximum":${UiOutlineFormat.READ_MAX},` +
    `"description":"默认 ${UiOutlineFormat.READ_DEFAULT}"}},"additionalProperties":false}`;
}
