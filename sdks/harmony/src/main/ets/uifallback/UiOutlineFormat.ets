// 进程内控件兜底的大纲与变化摘要格式：唯一定义见 spec/ui-fallback.md 第 4–7 节，本文件只是 ArkTS 实现
// （与 ArkUI 无关，可在 Node 上单元测试；结构对应 Android 的 UiOutlineFormat.kt）。

/** 条目类别：控件、分组（带引用，可用于 within）、标题（只作分组）。 */
export enum UiEntryKind {
  ITEM = 'item',
  CONTAINER = 'container',
  HEADING = 'heading',
}

/** {@link UiEntry} 的构造参数。 */
export interface UiEntryInit {
  kind: UiEntryKind;
  /** 前后两次快照中对应同一控件的键（控件与分组为引用）。 */
  key: string;
  role: string;
  label: string;
  ref?: string;
  name: string;
  value?: string;
  states: string[];
  required: boolean;
  declared?: string;
  level?: number;
  /** 所在分组层数（缩进）。 */
  depth: number;
  /** 祖先分组（分组与标题）在条目列表中的下标，外层在前。 */
  chain: number[];
  /** 祖先分组的键，外层在前。 */
  containers: string[];
}

/** 大纲中的一个条目。 */
export class UiEntry {
  readonly kind: UiEntryKind;
  readonly key: string;
  readonly role: string;
  readonly label: string;
  readonly ref?: string;
  readonly name: string;
  readonly value?: string;
  readonly states: string[];
  readonly required: boolean;
  /** 已声明的工具名；外层非控件节点上的声明在收集结束时补给子树中唯一的控件。 */
  declared?: string;
  readonly level?: number;
  readonly depth: number;
  readonly chain: number[];
  readonly containers: string[];

  constructor(init: UiEntryInit) {
    this.kind = init.kind;
    this.key = init.key;
    this.role = init.role;
    this.label = init.label;
    this.ref = init.ref;
    this.name = init.name;
    this.value = init.value;
    this.states = init.states;
    this.required = init.required;
    this.declared = init.declared;
    this.level = init.level;
    this.depth = init.depth;
    this.chain = init.chain;
    this.containers = init.containers;
  }

  /** `按钮「结算」`。 */
  get described(): string {
    return this.name === '' ? this.label : `${this.label}「${this.name}」`;
  }
}

/** `ui.outline` 结果中的结构化条目（spec/ui-fallback.md 4.3）。 */
export interface UiOutlineItem {
  ref: string;
  role: string;
  name: string;
  value?: string;
  states?: string[];
  required?: boolean;
  declared?: string;
  group?: string;
}

/** `ui.outline` 的结果（spec/ui-fallback.md 4.1）。 */
export interface UiOutline {
  text: string;
  items: UiOutlineItem[];
  total: number;
  remaining?: number;
  hint?: string;
}

/** 操作类工具的结果（spec/ui-fallback.md 7.2）。 */
export interface UiActionResult {
  ok: boolean;
  changes: string[];
  hint?: string;
}

/** `ui.read` 的结果。 */
export interface UiReadResult {
  ref: string;
  text: string;
  truncated: boolean;
}

const ROLE_LABELS: Record<string, string> = {
  'button': '按钮', 'link': '链接', 'textbox': '输入框', 'searchbox': '搜索框',
  'checkbox': '复选框', 'radio': '单选框', 'switch': '开关', 'combobox': '下拉框',
  'listbox': '列表框', 'option': '选项', 'slider': '滑块', 'spinbutton': '数字框',
  'tab': '标签页', 'menuitem': '菜单项', 'treeitem': '树节点', 'gridcell': '单元格',
  'status': '提示', 'alert': '警告', 'generic': '元素',
  'window': '窗口', 'dialog': '对话框', 'alertdialog': '警告框', 'navigation': '导航',
  'main': '主区域', 'form': '表单', 'search': '搜索区', 'group': '分组', 'list': '列表',
  'scrollable': '滚动区', 'heading': '标题',
};

const IGNORED_STATES: string[] = ['focused'];
const EXCLUSIVE_STATES: string[][] = [['checked', 'unchecked', 'mixed'], ['expanded', 'collapsed']];
const WHITESPACE: RegExp = new RegExp('\\s+', 'g');

export class UiOutlineFormat {
  /** 密码类控件的标签与固定掩码（spec/ui-fallback.md 8.1：不泄露长度）。 */
  static readonly SECURE_LABEL: string = '密码框';
  static readonly SECURE_MASK: string = '••••';
  static readonly NAME_MAX: number = 40;
  static readonly STATUS_NAME_MAX: number = 80;
  static readonly VALUE_MAX: number = 30;
  static readonly MAX_CHANGES: number = 15;
  static readonly LIMIT_MAX: number = 500;
  static readonly READ_DEFAULT: number = 1000;
  static readonly READ_MAX: number = 20000;
  /** 引用格式（spec/ui-fallback.md 第 3 节）。 */
  static readonly REF_PATTERN: RegExp = new RegExp('^e[1-9]\\d*$');

  static label(role: string): string {
    const l = ROLE_LABELS[role];
    return l !== undefined ? l : '元素';
  }

  static collapse(s: string | undefined | null): string {
    if (s === undefined || s === null) {
      return '';
    }
    return s.replace(WHITESPACE, ' ').trim();
  }

  /** 截断到 max 个码点（保留前 max − 1 个、去掉尾部空白再加 `…`）。 */
  static truncate(s: string, max: number): string {
    const points: string[] = Array.from(s);
    if (points.length <= max) {
      return s;
    }
    return points.slice(0, Math.max(0, max - 1)).join('').replace(new RegExp('\\s+$'), '') + '…';
  }

  private static groupLabel(e: UiEntry): string {
    return e.kind === UiEntryKind.HEADING ? e.name : e.described;
  }

  private static haystack(e: UiEntry, entries: UiEntry[]): string {
    const parts: string[] = [e.label, e.role, e.name, e.value ?? '', e.declared ?? ''];
    for (const i of e.chain) {
      parts.push(UiOutlineFormat.groupLabel(entries[i]));
    }
    return parts.join(' ').toLowerCase();
  }

  private static itemLine(e: UiEntry): string {
    let line = `${e.ref ?? ''} ${e.described}`;
    if (e.value !== undefined) {
      line += `= "${e.value}"`;
    }
    if (e.states.length > 0) {
      line += ' ' + e.states.join(' ');
    }
    if (e.required) {
      line += ' (必填)';
    }
    if (e.declared !== undefined) {
      line += ` [已声明：${e.declared}]`;
    }
    return line;
  }

  /** 过滤、截断、分组并渲染大纲（spec/ui-fallback.md 4.2 / 4.3）。 */
  static render(entries: UiEntry[], query: string | undefined, limit: number): UiOutline {
    const tokens: string[] = (query ?? '').toLowerCase().split(WHITESPACE).filter((t: string) => t !== '');
    const matched: number[] = [];
    entries.forEach((e: UiEntry, i: number) => {
      if (e.kind !== UiEntryKind.ITEM) {
        return;
      }
      const h = tokens.length === 0 ? '' : UiOutlineFormat.haystack(e, entries);
      if (tokens.every((t: string) => h.includes(t))) {
        matched.push(i);
      }
    });
    const shown = new Set<number>(matched.slice(0, limit));
    const keptGroups = new Set<number>();
    shown.forEach((i: number) => entries[i].chain.forEach((c: number) => keptGroups.add(c)));
    const lines: string[] = [];
    const items: UiOutlineItem[] = [];
    let declared = false;
    entries.forEach((e: UiEntry, i: number) => {
      const indent = ' '.repeat(e.depth * 2);
      if (e.kind === UiEntryKind.ITEM) {
        if (!shown.has(i)) {
          return;
        }
        lines.push(indent + UiOutlineFormat.itemLine(e));
        declared = declared || e.declared !== undefined;
        const group = e.chain.map((c: number) => UiOutlineFormat.groupLabel(entries[c])).join(' › ');
        const item: UiOutlineItem = { ref: e.ref ?? '', role: e.role, name: e.name };
        if (e.value !== undefined) {
          item.value = e.value;
        }
        if (e.states.length > 0) {
          item.states = e.states;
        }
        if (e.required) {
          item.required = true;
        }
        if (e.declared !== undefined) {
          item.declared = e.declared;
        }
        if (group !== '') {
          item.group = group;
        }
        items.push(item);
      } else if (!keptGroups.has(i)) {
        return;
      } else if (e.kind === UiEntryKind.CONTAINER) {
        lines.push(indent + `» ${e.ref ?? ''} ${e.described}` + (e.declared !== undefined ? ` [已声明：${e.declared}]` : ''));
        declared = declared || e.declared !== undefined;
      } else {
        lines.push(indent + '#'.repeat(e.level ?? 2) + ' ' + e.name);
      }
    });
    const remaining = matched.length - shown.size;
    if (lines.length === 0) {
      lines.push(tokens.length > 0 ? `（没有与「${query ?? ''}」匹配的可交互元素）` : '（没有可见的可交互元素）');
    }
    if (remaining > 0) {
      lines.push(`…另有 ${remaining} 个元素未列出，可用 query 或 within 缩小范围`);
    }
    const outline: UiOutline = { text: lines.join('\n'), items: items, total: matched.length };
    if (remaining > 0) {
      outline.remaining = remaining;
    }
    if (declared) {
      outline.hint = '标注 [已声明：…] 的元素已有对应工具，请优先直接调用该工具';
    }
    return outline;
  }

  private static stateChange(before: string[], after: string[]): string[] {
    const b = before.filter((s: string) => !IGNORED_STATES.includes(s));
    const a = after.filter((s: string) => !IGNORED_STATES.includes(s));
    const added = a.filter((s: string) => !b.includes(s));
    const removed = b.filter((s: string) => !a.includes(s)).filter((r: string) =>
      !EXCLUSIVE_STATES.some((g: string[]) => g.includes(r) && added.some((x: string) => g.includes(x))));
    const out: string[] = [];
    if (added.length > 0) {
      out.push(`变为 ${added.join('、')}`);
    }
    if (removed.length > 0) {
      out.push(`不再 ${removed.join('、')}`);
    }
    return out;
  }

  private static visibleStates(e: UiEntry): string {
    const s = e.states.filter((x: string) => !IGNORED_STATES.includes(x));
    return s.length === 0 ? '' : ' ' + s.join(' ');
  }

  private static changeLabel(e: UiEntry): string {
    return e.kind === UiEntryKind.HEADING ? `标题「${e.name}」` : `${e.described}(${e.ref ?? ''})`;
  }

  /** 外层最先出现的"同样是新增 / 同样被移除"的祖先分组。 */
  private static outerChanged(e: UiEntry, set: Map<string, UiEntry>, other: Map<string, UiEntry>): string | undefined {
    return e.containers.find((c: string) => set.has(c) && !other.has(c));
  }

  private static childCounts(list: UiEntry[], set: Map<string, UiEntry>, other: Map<string, UiEntry>): Map<string, number> {
    const counts = new Map<string, number>();
    for (const e of list) {
      if (other.has(e.key)) {
        continue;
      }
      const outer = UiOutlineFormat.outerChanged(e, set, other);
      if (outer === undefined) {
        continue;
      }
      counts.set(outer, (counts.get(outer) ?? 0) + (e.kind === UiEntryKind.ITEM ? 1 : 0));
    }
    return counts;
  }

  private static byKey(list: UiEntry[]): Map<string, UiEntry> {
    const m = new Map<string, UiEntry>();
    list.forEach((e: UiEntry) => m.set(e.key, e));
    return m;
  }

  /** 操作前后的变化摘要（spec/ui-fallback.md 7.2）。 */
  static diff(before: UiEntry[], after: UiEntry[], prefix: string): string[] {
    const byBefore = UiOutlineFormat.byKey(before);
    const byAfter = UiOutlineFormat.byKey(after);
    const added = UiOutlineFormat.childCounts(after, byAfter, byBefore);
    const removed = UiOutlineFormat.childCounts(before, byBefore, byAfter);
    const output: string[] = [];
    for (const e of after) {
      const old = byBefore.get(e.key);
      if (old === undefined) {
        if (UiOutlineFormat.outerChanged(e, byAfter, byBefore) !== undefined) {
          continue;
        }
        let line = `新增${UiOutlineFormat.changeLabel(e)}`;
        if (e.kind === UiEntryKind.ITEM) {
          if (e.value !== undefined) {
            line += ` = "${e.value}"`;
          }
          line += UiOutlineFormat.visibleStates(e);
        }
        const n = added.get(e.key) ?? 0;
        if (n > 0) {
          line += `，含 ${n} 个可交互元素（可用 ${prefix}.outline({ within: "${e.ref ?? ''}" }) 查看）`;
        }
        output.push(line);
        continue;
      }
      if (e.kind === UiEntryKind.HEADING) {
        if (old.name !== e.name) {
          output.push(`标题「${old.name}」变为「${e.name}」`);
        }
        continue;
      }
      const parts: string[] = [];
      if (old.name !== e.name) {
        parts.push(`名称变为「${e.name}」`);
      }
      if (old.value !== e.value) {
        parts.push(e.value !== undefined ? `值变为 "${e.value}"` : '值已清空');
      }
      UiOutlineFormat.stateChange(old.states, e.states).forEach((p: string) => parts.push(p));
      if (parts.length > 0) {
        output.push(`${e.ref ?? ''} ${old.name !== '' ? old.name : old.label} ${parts.join('，')}`);
      }
    }
    for (const e of before) {
      if (byAfter.has(e.key) || UiOutlineFormat.outerChanged(e, byBefore, byAfter) !== undefined) {
        continue;
      }
      const n = removed.get(e.key) ?? 0;
      output.push(`${UiOutlineFormat.changeLabel(e)} 已消失` + (n > 0 ? `（含 ${n} 个可交互元素）` : ''));
    }
    if (output.length <= UiOutlineFormat.MAX_CHANGES) {
      return output;
    }
    const rest = output.length - UiOutlineFormat.MAX_CHANGES;
    const head = output.slice(0, UiOutlineFormat.MAX_CHANGES);
    head.push(`…另有 ${rest} 项变化，请调用 ${prefix}.outline 查看`);
    return head;
  }

  /** `ui.read` 的文本：折叠空白、相邻重复只保留一次、截断到 max 个码点。 */
  static readText(parts: string[], reference: string | undefined, max: number): UiReadResult {
    const texts = parts.map((p: string) => UiOutlineFormat.collapse(p)).filter((s: string) => s !== '');
    const full = texts.filter((s: string, i: number) => i === 0 || s !== texts[i - 1]).join(' ');
    const truncated = Array.from(full).length > max;
    return { ref: reference ?? 'root', text: truncated ? UiOutlineFormat.truncate(full, max) : full, truncated: truncated };
  }
}
