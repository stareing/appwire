// 控件树的平台无关部分：节点描述、元素接口、引用登记与大纲收集（spec/ui-fallback.md 第 3–6 节）；
// 对应 Android 的 UiTree.kt。ArkUI 适配见 ArkUiElements.ets。

import { UiEntry, UiEntryKind, UiOutlineFormat } from './UiOutlineFormat';
import { UiKey, UiScrollDirection } from './UiFallbackInput';

/**
 * 控件节点的描述（spec/ui-fallback.md 第 4–6 节）。
 * `kind` 为 undefined = 不列出、只遍历子节点（布局容器等），此时只有 `declared` 有意义；
 * `descend`：控件（ITEM）是否仍遍历子节点（可点击的布局容器里还有按钮）。
 */
export interface UiDescription {
  kind?: UiEntryKind;
  role: string;
  name: string;
  /** 原始值（未截断）；`secure` 时不读取，只看 `hasSecureValue`。 */
  value?: string;
  states: string[];
  required: boolean;
  secure: boolean;
  hasSecureValue: boolean;
  declared?: string;
  level?: number;
  descend: boolean;
}

/** 不列出、只遍历子节点的描述。 */
export function transparentDescription(declared?: string): UiDescription {
  const d: UiDescription = {
    role: 'generic', name: '', states: [], required: false, secure: false, hasSecureValue: false, descend: true,
  };
  d.declared = declared;
  return d;
}

/** 动作类别（用于说明不支持的原因）。 */
export type UiAction = 'click' | 'fill' | 'scroll';

/**
 * 兜底遍历的控件节点（平台适配：ArkUI 检查器树节点）。只在 UI 线程调用。
 * 动作方法返回 false 表示该控件不支持此动作；需要更具体的说明时抛 ToolCallError（用 UiFallbackInput 构造）。
 */
export interface UiElement {
  /** 引用的键：同一控件多次遍历得到的值相同。 */
  identity(): string;
  /** 框架会把同一控件复用给另一个数据项（懒加载列表）：引用按指纹核对，指纹变了即作废。 */
  reusable(): boolean;
  /** 本身（连同子树）不可见：隐藏、零尺寸、被裁剪或滚出视口。 */
  isHidden(): boolean;
  describe(): UiDescription;
  children(): UiElement[];
  /** `ui.read` 中本节点自身的文本（名称、值；密码类只给掩码）。 */
  readText(): string[];
  /** `ui.read` 是否继续读子节点。 */
  readChildren(): boolean;
  isEnabled(): boolean;
  /** 激活：点击、切换、选中。 */
  click(): boolean;
  setText(text: string): boolean;
  selectOption(text: string): boolean;
  setRange(value: number): boolean;
  focus(): boolean;
  /** 文本框的输入法动作（完成 / 提交）。 */
  imeAction(): boolean;
  /** 平台是否能滚动本控件（或其所在滚动区）；不能时 `ui.scroll` 返回 unsupported。 */
  canScroll(): boolean;
  scrollIntoView(): boolean;
  scrollPage(direction: UiScrollDirection): boolean;
  /** 动作不被支持时给模型的原因（如"没有设置 .id()"）；没有时为 undefined。 */
  unsupportedReason(action: UiAction): string | undefined;
}

/** 顶层窗口 / 对话框：作为 `window` / `dialog` 分组，并承接按键。 */
export interface UiWindow extends UiElement {
  /** 把按键（按下 + 抬起）交给焦点控件；返回是否被处理。 */
  sendKey(key: UiKey, target?: UiElement): boolean;
  /** 焦点移到下一个 / 上一个控件。 */
  moveFocus(forward: boolean): boolean;
  /** Escape 未被处理时的关闭 / 取消。 */
  dismiss(): boolean;
}

/** 一个可见节点及其条目。 */
export class UiNode {
  readonly element: UiElement;
  readonly entry: UiEntry;
  readonly secure: boolean;
  readonly window: UiWindow;

  constructor(element: UiElement, entry: UiEntry, secure: boolean, window: UiWindow) {
    this.element = element;
    this.entry = entry;
    this.secure = secure;
    this.window = window;
  }

  get label(): string {
    return `${this.entry.ref ?? ''} ${this.entry.described}`;
  }
}

/** 一次收集的结果：条目（按树序）与引用 → 节点。 */
export class UiSnapshot {
  readonly entries: UiEntry[];
  readonly byRef: Map<string, UiNode>;

  constructor(entries: UiEntry[], byRef: Map<string, UiNode>) {
    this.entries = entries;
    this.byRef = byRef;
  }
}

class RefRecord {
  identity: string;
  readonly fingerprint: string;

  constructor(identity: string, fingerprint: string) {
    this.identity = identity;
    this.fingerprint = fingerprint;
  }
}

/**
 * 引用 `eN` ↔ 控件（spec/ui-fallback.md 第 3 节）：键 + 指纹（角色 + 名称 + 所在分组）。
 * 控件被框架重建时，按指纹在当前界面中唯一匹配到的新控件沿用原引用；可复用的控件指纹变了时旧引用作废。
 *
 * @invariant 只在 UI 线程访问。
 */
export class UiRefRegistry {
  private readonly records: Map<string, RefRecord> = new Map<string, RefRecord>();
  private next: number = 1;

  private find(identity: string): string | undefined {
    for (const entry of this.records) {
      if (entry[1].identity === identity) {
        return entry[0];
      }
    }
    return undefined;
  }

  /**
   * 取得（必要时分配或按指纹沿用）控件的引用。`verify` 为 false 时不核对指纹；`live` 判断旧控件是否仍在界面上。
   */
  refOf(identity: string, fingerprint: string, taken: Set<string>, fingerprintCount: Map<string, number>,
    verify: boolean, live: (identity: string) => boolean): string {
    const existing = this.find(identity);
    if (existing !== undefined) {
      if (!verify || this.records.get(existing)?.fingerprint === fingerprint) {
        return existing;
      }
      // @why 框架把控件复用给了另一个数据项：旧引用作废，不能让它指向新控件。
      this.records.delete(existing);
    }
    if (fingerprintCount.get(fingerprint) === 1) {
      const orphans: string[] = [];
      this.records.forEach((rec: RefRecord, r: string) => {
        if (!taken.has(r) && rec.fingerprint === fingerprint && (rec.identity === identity || !live(rec.identity))) {
          orphans.push(r);
        }
      });
      if (orphans.length === 1) {
        const rec = this.records.get(orphans[0]);
        if (rec !== undefined) {
          rec.identity = identity;
        }
        return orphans[0];
      }
    }
    const r = `e${this.next++}`;
    this.records.set(r, new RefRecord(identity, fingerprint));
    return r;
  }

  /** 引用记录的控件键（控件可能已不在界面上）。 */
  target(reference: string): string | undefined {
    return this.records.get(reference)?.identity;
  }

  /** 丢弃控件已不在界面上的记录。 */
  prune(live: (identity: string) => boolean): void {
    const dead: string[] = [];
    this.records.forEach((rec: RefRecord, r: string) => {
      if (!live(rec.identity)) {
        dead.push(r);
      }
    });
    dead.forEach((r: string) => this.records.delete(r));
  }

  clear(): void {
    this.records.clear();
  }
}

class RawEntry {
  readonly element: UiElement;
  readonly window: UiWindow;
  readonly desc: UiDescription;
  readonly kind: UiEntryKind;
  readonly name: string;
  readonly path: string;
  readonly depth: number;
  readonly chain: number[];
  readonly containers: number[];

  constructor(element: UiElement, window: UiWindow, desc: UiDescription, kind: UiEntryKind, name: string, path: string,
    depth: number, chain: number[], containers: number[]) {
    this.element = element;
    this.window = window;
    this.desc = desc;
    this.kind = kind;
    this.name = name;
    this.path = path;
    this.depth = depth;
    this.chain = chain;
    this.containers = containers;
  }
}

class HeadingMark {
  readonly level: number;
  readonly index: number;

  constructor(level: number, index: number) {
    this.level = level;
    this.index = index;
  }
}

class Frame {
  readonly container: number;
  readonly headings: HeadingMark[] = [];

  constructor(container: number) {
    this.container = container;
  }
}

class DeclaredGroup {
  readonly tool: string;
  readonly from: number;
  readonly to: number;

  constructor(tool: string, from: number, to: number) {
    this.tool = tool;
    this.from = from;
    this.to = to;
  }
}

/** 从窗口收集兜底大纲条目（spec/ui-fallback.md 第 4 节）。 */
export class UiTreeCollector {
  private static nameOf(d: UiDescription): string {
    const max = d.role === 'status' || d.role === 'alert' ? UiOutlineFormat.STATUS_NAME_MAX : UiOutlineFormat.NAME_MAX;
    return UiOutlineFormat.truncate(UiOutlineFormat.collapse(d.name), max);
  }

  private static valueOf(d: UiDescription): string | undefined {
    if (d.secure) {
      return d.hasSecureValue ? UiOutlineFormat.SECURE_MASK : undefined;
    }
    const v = UiOutlineFormat.collapse(d.value);
    return v === '' ? undefined : UiOutlineFormat.truncate(v, UiOutlineFormat.VALUE_MAX);
  }

  /**
   * 收集 `windows`（或 `start` 子树）下可见的条目，并为控件与分组分配引用。
   * `start` 给出时不按指纹沿用 / 核对引用（分组路径不完整）。
   */
  static collect(refs: UiRefRegistry, windows: UiWindow[], live: (identity: string) => boolean,
    start?: UiNode): UiSnapshot {
    const full = start === undefined;
    const raw: RawEntry[] = [];
    const declaredGroups: DeclaredGroup[] = [];
    const frames: Frame[] = [new Frame(-1)];
    const containerStack: number[] = [];

    const chain = (): number[] => {
      const c: number[] = [];
      frames.forEach((f: Frame) => {
        if (f.container >= 0) {
          c.push(f.container);
        }
        f.headings.forEach((h: HeadingMark) => c.push(h.index));
      });
      return c;
    };
    const path = (c: number[]): string => c.map((i: number) => {
      const r = raw[i];
      return r.kind === UiEntryKind.HEADING ? r.name : `${UiOutlineFormat.label(r.desc.role)}「${r.name}」`;
    }).join(' › ');

    const walk = (e: UiElement, window: UiWindow): void => {
      if (e.isHidden()) {
        return;
      }
      const d = e.describe();
      const kind = d.kind;
      let pushed = false;
      const declaredFrom = raw.length;
      if (kind === UiEntryKind.HEADING) {
        const name = UiTreeCollector.nameOf(d);
        if (name !== '') {
          const frame = frames[frames.length - 1];
          const level = Math.min(3, Math.max(1, d.level ?? 2));
          while (frame.headings.length > 0 && frame.headings[frame.headings.length - 1].level >= level) {
            frame.headings.pop();
          }
          const c = chain();
          const hd: UiDescription = {
            kind: UiEntryKind.HEADING, role: 'heading', name: name, states: [], required: false, secure: false,
            hasSecureValue: false, descend: true, level: level,
          };
          raw.push(new RawEntry(e, window, hd, UiEntryKind.HEADING, name, path(c), frames.length - 1, c,
            containerStack.slice()));
          frame.headings.push(new HeadingMark(level, raw.length - 1));
        }
      } else if (kind !== undefined) {
        const c = chain();
        raw.push(new RawEntry(e, window, d, kind, UiTreeCollector.nameOf(d), path(c), frames.length - 1, c,
          containerStack.slice()));
        if (kind === UiEntryKind.CONTAINER) {
          frames.push(new Frame(raw.length - 1));
          containerStack.push(raw.length - 1);
          pushed = true;
        }
      }
      if (kind !== UiEntryKind.ITEM || d.descend) {
        e.children().forEach((child: UiElement) => walk(child, window));
      }
      if (pushed) {
        frames.pop();
        containerStack.pop();
      }
      if (kind === undefined && d.declared !== undefined) {
        declaredGroups.push(new DeclaredGroup(d.declared, declaredFrom, raw.length));
      }
    };

    if (start !== undefined) {
      walk(start.element, start.window);
    } else {
      windows.forEach((w: UiWindow) => walk(w, w));
    }

    // 第二遍：指纹计数 → 分配引用 → 条目。
    const fingerprintOf = (r: RawEntry): string => `${r.desc.role}|${r.name}|${r.path}`;
    const counts = new Map<string, number>();
    raw.forEach((r: RawEntry) => {
      if (r.kind !== UiEntryKind.HEADING) {
        const f = fingerprintOf(r);
        counts.set(f, (counts.get(f) ?? 0) + 1);
      }
    });
    const taken = new Set<string>();
    const refsByIndex: (string | undefined)[] = [];
    raw.forEach((r: RawEntry) => {
      if (r.kind === UiEntryKind.HEADING) {
        refsByIndex.push(undefined);
        return;
      }
      const ref = refs.refOf(r.element.identity(), fingerprintOf(r), taken,
        full ? counts : new Map<string, number>(), full && r.element.reusable(), live);
      taken.add(ref);
      refsByIndex.push(ref);
    });
    const keyOf = (i: number): string => refsByIndex[i] ?? `h:${raw[i].element.identity()}`;

    const entries: UiEntry[] = [];
    const byRef = new Map<string, UiNode>();
    raw.forEach((r: RawEntry, i: number) => {
      const d = r.desc;
      const isItem = r.kind === UiEntryKind.ITEM;
      const entry = new UiEntry({
        kind: r.kind,
        key: keyOf(i),
        role: d.role,
        label: d.secure ? UiOutlineFormat.SECURE_LABEL : UiOutlineFormat.label(d.role),
        ref: refsByIndex[i],
        name: r.name,
        value: isItem ? UiTreeCollector.valueOf(d) : undefined,
        states: isItem ? d.states : [],
        required: isItem && d.required,
        declared: r.kind === UiEntryKind.HEADING ? undefined : d.declared,
        level: r.kind === UiEntryKind.HEADING ? d.level : undefined,
        depth: r.depth,
        chain: r.chain,
        containers: r.containers.map((c: number) => keyOf(c)),
      });
      entries.push(entry);
      const ref = refsByIndex[i];
      if (ref !== undefined) {
        byRef.set(ref, new UiNode(r.element, entry, d.secure, r.window));
      }
    });
    // 非控件节点上的声明：子树中恰好一个控件时标给它。
    declaredGroups.forEach((g: DeclaredGroup) => {
      const items = entries.slice(g.from, g.to).filter((e: UiEntry) => e.kind === UiEntryKind.ITEM);
      if (items.length === 1 && items[0].declared === undefined) {
        items[0].declared = g.tool;
      }
    });
    if (full) {
      refs.prune(live);
    }
    return new UiSnapshot(entries, byRef);
  }

  /** 按引用在全部窗口中（含不可见的节点）找到仍在界面上的控件。 */
  static findAttached(refs: UiRefRegistry, reference: string, windows: UiWindow[]): UiElement | undefined {
    const identity = refs.target(reference);
    if (identity === undefined) {
      return undefined;
    }
    const find = (e: UiElement): UiElement | undefined => {
      if (e.identity() === identity) {
        return e;
      }
      for (const c of e.children()) {
        const hit = find(c);
        if (hit !== undefined) {
          return hit;
        }
      }
      return undefined;
    };
    for (const w of windows) {
      const hit = find(w);
      if (hit !== undefined) {
        return hit;
      }
    }
    return undefined;
  }
}
