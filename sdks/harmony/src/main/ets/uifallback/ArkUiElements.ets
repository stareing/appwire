// ArkUI 适配：把检查器树（UIContext.getFilteredInspectorTree() 的 JSON）映射为兜底元素，动作经窗口驱动
// （ArkUiWindowDriver，缺省实现见 ArkUiDriver.ets）按组件 id / uniqueId 派发。不截图、不按坐标点击。
//
// @compat 检查器 JSON 的字段（$type / $ID / $rect / $attrs / $children 与各组件的属性名）来自 ArkUI 检查器的输出约定，
//   d.ts 只声明返回 JSON 字符串；本文件按字段缺省时的保守取值处理（未知组件不列出），真机格式需在设备上核对。

import { UiFallbackInput, UiKey, UiScrollDirection } from './UiFallbackInput';
import { UiEntryKind, UiOutlineFormat } from './UiOutlineFormat';
import { UiAction, UiDescription, UiElement, UiWindow, transparentDescription } from './UiTree';
import { UiPlatform } from './UiInspector';

/** 标注已声明工具的自定义属性名：`.customProperty(MCP_DECLARED_PROPERTY, 'cart.clear')`。 */
export const MCP_DECLARED_PROPERTY: string = 'appMcpTool';

/**
 * 一个窗口（主窗口或子窗口的 UIContext）的检查器与动作入口。缺省实现 {@link UIContextDriver}；测试用假实现。
 */
export interface ArkUiWindowDriver {
  /** 窗口名（大纲中 `窗口「…」` 的名称）。 */
  readonly title: string;
  /** 检查器树 JSON（`UIContext.getFilteredInspectorTree()`）。 */
  inspectorTree(): string;
  /** 组件的自定义属性（`FrameNode.getCustomProperty`）；没有时 undefined。 */
  customProperty(uniqueId: number, name: string): string | undefined;
  /** 组件是否绑定了 onClick（`FrameNode.getInteractionEventBindingInfo(EventQueryType.ON_CLICK)`）。 */
  hasClickHandler(uniqueId: number): boolean;
  /** 按组件 id 派发点击（`sendEventByKey(id, 10, '')`）；返回是否送达。 */
  clickById(inspectorId: string): boolean;
  /** 按组件 id 请求焦点（`FocusController.requestFocus`）；失败时 false。 */
  requestFocus(inspectorId: string): boolean;
  /** 把按键（按下 + 抬起）派发给组件（`UIContext.dispatchKeyEvent`）；返回是否被消费。 */
  dispatchKey(uniqueId: number, key: UiKey): boolean;
}

/** 检查器 JSON 节点。 */
export interface InspectorJson {
  $type?: string;
  $ID?: number;
  $rect?: string;
  $attrs?: Record<string, Object>;
  $children?: InspectorJson[];
  width?: string;
  height?: string;
}

/** 矩形（窗口像素）。 */
export class UiRect {
  readonly left: number;
  readonly top: number;
  readonly right: number;
  readonly bottom: number;

  constructor(left: number, top: number, right: number, bottom: number) {
    this.left = left;
    this.top = top;
    this.right = right;
    this.bottom = bottom;
  }

  get empty(): boolean {
    return this.right <= this.left || this.bottom <= this.top;
  }

  intersect(o: UiRect): UiRect {
    return new UiRect(Math.max(this.left, o.left), Math.max(this.top, o.top), Math.min(this.right, o.right),
      Math.min(this.bottom, o.bottom));
  }

  /** 解析 `"[x1, y1],[x2, y2]"`；格式不符时 undefined。 */
  static parse(text: string | undefined): UiRect | undefined {
    if (text === undefined) {
      return undefined;
    }
    const nums = text.match(new RegExp('-?\\d+(\\.\\d+)?', 'g'));
    if (nums === null || nums.length < 4) {
      return undefined;
    }
    return new UiRect(Number(nums[0]), Number(nums[1]), Number(nums[2]), Number(nums[3]));
  }
}

/** 对话框类覆盖层的组件类型（遮挡同窗口的其余内容）。 */
const DIALOG_TYPES: string[] = ['Dialog', 'CustomDialog', 'AlertDialog', 'ActionSheet', 'MenuWrapper', 'SheetWrapper',
  'SheetPage', 'ModalPage', 'Popup'];
/** 裁剪子组件、子组件可滚出视口的容器。 */
const CLIP_TYPES: string[] = ['Scroll', 'List', 'Grid', 'WaterFlow', 'Swiper', 'Tabs', 'ArcList'];
/** 懒加载 / 复用子组件的容器（引用按指纹核对）。 */
const REUSE_TYPES: string[] = ['List', 'Grid', 'WaterFlow', 'ArcList'];
const TEXT_TYPES: string[] = ['TextInput', 'TextArea', 'RichEditor', 'Search'];
const LABEL_TYPES: string[] = ['Text', 'Span', 'SymbolSpan'];

const NO_ID_REASON: string = '该组件没有设置 .id()，ArkUI 只能按组件 id 派发点击；请设置 id 或声明语义工具';
const NO_TEXT_REASON: string = 'ArkUI 没有从组件外写入输入框文本的接口，请为该输入框声明语义工具';
const NO_SCROLL_REASON: string = 'ArkUI 没有从组件外滚动任意容器的接口（Scroller 由页面持有），请声明语义工具';
const NO_SELECT_REASON: string = 'ArkUI 没有从组件外选中下拉项或设置滑块的接口；下拉框请先 click 展开再 click 选项';

function attrString(attrs: Record<string, Object>, key: string): string | undefined {
  const v: Object | undefined | null = attrs[key];
  if (v === undefined || v === null) {
    return undefined;
  }
  return typeof v === 'string' ? v as string : JSON.stringify(v);
}

function attrBool(attrs: Record<string, Object>, key: string): boolean | undefined {
  const v = attrString(attrs, key);
  if (v === undefined) {
    return undefined;
  }
  return v === 'true' ? true : v === 'false' ? false : undefined;
}

/** 一次刷新内的共享状态：窗口驱动、窗口序号、存活的组件键。 */
class WindowScope {
  readonly driver: ArkUiWindowDriver;
  readonly index: number;
  readonly live: Set<string> = new Set<string>();
  /** 本窗口的组件（树序），用于 Tab 焦点移动。 */
  readonly all: ArkUiElement[] = [];

  constructor(driver: ArkUiWindowDriver, index: number) {
    this.driver = driver;
    this.index = index;
  }
}

/** 检查器树节点上的兜底元素。 */
export class ArkUiElement implements UiElement {
  readonly json: InspectorJson;
  protected readonly scope: WindowScope;
  protected readonly attrs: Record<string, Object>;
  readonly type: string;
  private readonly key: string;
  private readonly clip: UiRect | undefined;
  private readonly inReuse: boolean;
  private kids: ArkUiElement[] | undefined = undefined;
  private description: UiDescription | undefined = undefined;

  constructor(json: InspectorJson, scope: WindowScope, path: string, clip: UiRect | undefined, inReuse: boolean) {
    this.json = json;
    this.scope = scope;
    this.attrs = json.$attrs ?? {};
    this.type = json.$type ?? '';
    this.key = `${scope.index}:` + (json.$ID !== undefined ? String(json.$ID) : path);
    this.clip = clip;
    this.inReuse = inReuse;
    scope.live.add(this.key);
    scope.all.push(this);
  }

  /** 组件的 uniqueId（检查器 `$ID`）；没有时 -1。 */
  get uniqueId(): number {
    return this.json.$ID ?? -1;
  }

  /** 组件 id（`.id()` / `.key()`）；没有时空串。 */
  get inspectorId(): string {
    return attrString(this.attrs, 'id') ?? '';
  }

  get rect(): UiRect | undefined {
    return UiRect.parse(this.json.$rect);
  }

  identity(): string {
    return this.key;
  }

  reusable(): boolean {
    return this.inReuse;
  }

  isHidden(): boolean {
    const visibility = attrString(this.attrs, 'visibility');
    if (visibility === 'Visibility.Hidden' || visibility === 'Visibility.None') {
      return true;
    }
    const opacity = attrString(this.attrs, 'opacity');
    if (opacity !== undefined && Number(opacity) === 0) {
      return true;
    }
    const r = this.rect;
    if (r === undefined) {
      return false;
    }
    return r.empty || (this.clip !== undefined && r.intersect(this.clip).empty);
  }

  children(): UiElement[] {
    return this.childElements();
  }

  /** 子元素（首次访问时构造并缓存）。 */
  childElements(): ArkUiElement[] {
    if (this.kids === undefined) {
      const r = this.rect;
      let clip = this.clip;
      if (CLIP_TYPES.includes(this.type) && r !== undefined) {
        clip = clip !== undefined ? clip.intersect(r) : r;
      }
      const reuse = this.inReuse || REUSE_TYPES.includes(this.type);
      const kids: ArkUiElement[] = [];
      (this.json.$children ?? []).forEach((c: InspectorJson, i: number) => {
        kids.push(new ArkUiElement(c, this.scope, `${this.key}/${i}`, clip, reuse));
      });
      this.kids = kids;
    }
    return this.kids;
  }

  /** 子树中可见文本组件的内容（作名称）。 */
  private descendantText(): string {
    const parts: string[] = [];
    const walk = (e: ArkUiElement): void => {
      if (e.isHidden()) {
        return;
      }
      if (LABEL_TYPES.includes(e.type)) {
        const c = attrString(e.attrs, 'content');
        if (c !== undefined && c !== '') {
          parts.push(c);
        }
      }
      e.childElements().forEach((k: ArkUiElement) => walk(k));
    };
    this.childElements().forEach((k: ArkUiElement) => walk(k));
    return parts.join(' ');
  }

  private declaredTool(): string | undefined {
    if (this.uniqueId < 0) {
      return undefined;
    }
    const v = this.scope.driver.customProperty(this.uniqueId, MCP_DECLARED_PROPERTY);
    return v !== undefined && v !== '' ? v : undefined;
  }

  private firstOf(...names: (string | undefined)[]): string {
    for (const n of names) {
      if (n !== undefined && n !== '') {
        return n;
      }
    }
    return '';
  }

  private item(role: string, name: string, value?: string, states?: string[], descend?: boolean): UiDescription {
    const s: string[] = states ?? [];
    if (attrBool(this.attrs, 'focused') === true) {
      s.push('focused');
    }
    if (!this.isEnabled()) {
      s.unshift('disabled');
    }
    const d: UiDescription = {
      kind: UiEntryKind.ITEM, role: role, name: name, states: s, required: false, secure: false,
      hasSecureValue: false, descend: descend ?? false,
    };
    d.value = value;
    d.declared = this.declaredTool();
    return d;
  }

  private container(role: string, name: string): UiDescription {
    const d: UiDescription = {
      kind: UiEntryKind.CONTAINER, role: role, name: name, states: [], required: false, secure: false,
      hasSecureValue: false, descend: true,
    };
    d.declared = this.declaredTool();
    return d;
  }

  private checked(on: boolean | undefined): string[] {
    return [on === true ? 'checked' : 'unchecked'];
  }

  /** 输入框类组件：密码类只给掩码（spec/ui-fallback.md 8.1）。 */
  private textField(): UiDescription {
    const role = this.type === 'Search' ? 'searchbox' : 'textbox';
    const value = attrString(this.attrs, this.type === 'Search' ? 'value' : 'text') ?? '';
    const name = this.firstOf(attrString(this.attrs, 'accessibilityText'), attrString(this.attrs, 'placeholder'));
    const states: string[] = [];
    if (attrBool(this.attrs, 'editable') === false) {
      states.push('readonly');
    }
    if (this.isSecure()) {
      const d = this.item(role, name, undefined, states);
      d.secure = true;
      d.hasSecureValue = value !== '';
      return d;
    }
    return this.item(role, name, value, states);
  }

  /** 密码类输入框：`type` 为 InputType.Password / NEW_PASSWORD / NUMBER_PASSWORD。 */
  isSecure(): boolean {
    return TEXT_TYPES.includes(this.type) && (attrString(this.attrs, 'type') ?? '').toLowerCase().includes('password');
  }

  private classify(): UiDescription {
    const a = this.attrs;
    const label = (): string => this.firstOf(attrString(a, 'accessibilityText'), attrString(a, 'label'),
      attrString(a, 'content'), this.descendantText());
    switch (this.type) {
      case 'Button':
        return this.item('button', label());
      case 'Toggle': {
        const type = attrString(a, 'type') ?? '';
        const on = attrBool(a, 'isOn');
        if (type.includes('Checkbox')) {
          return this.item('checkbox', label(), undefined, this.checked(on));
        }
        if (type.includes('Button')) {
          return this.item('button', label(), undefined, on === true ? ['pressed'] : []);
        }
        return this.item('switch', label(), undefined, this.checked(on));
      }
      case 'Checkbox':
        return this.item('checkbox', label(), undefined, this.checked(attrBool(a, 'select')));
      case 'CheckboxGroup':
        return this.item('checkbox', label(), undefined, this.checked(attrBool(a, 'selectAll')));
      case 'Radio':
        return this.item('radio', label(), undefined, this.checked(attrBool(a, 'checked')));
      case 'TextInput':
      case 'TextArea':
      case 'RichEditor':
      case 'Search':
        return this.textField();
      case 'Select':
        return this.item('combobox', this.firstOf(attrString(a, 'accessibilityText')), attrString(a, 'value'));
      case 'Slider':
        return this.item('slider', this.firstOf(attrString(a, 'accessibilityText')), attrString(a, 'value'));
      case 'Rating':
        return this.item('slider', this.firstOf(attrString(a, 'accessibilityText')), attrString(a, 'rating'));
      case 'Hyperlink':
        return this.item('link', label());
      case 'MenuItem':
        return this.item('menuitem', label());
      case 'Navigation':
        return this.container('navigation', this.firstOf(attrString(a, 'title')));
      case 'NavDestination':
        return this.container('main', this.firstOf(attrString(a, 'title')));
      case 'Scroll':
        return this.container('scrollable', '');
      case 'List':
      case 'Grid':
      case 'WaterFlow':
        return this.container('list', '');
      default:
        break;
    }
    if (this.uniqueId >= 0 && this.scope.driver.hasClickHandler(this.uniqueId)) {
      // 绑定了 onClick 的文本 / 图片 / 布局容器：作为按钮列出，并继续遍历其中的控件。
      return this.item('button', label(), undefined, [], true);
    }
    return transparentDescription(this.declaredTool());
  }

  describe(): UiDescription {
    if (this.description === undefined) {
      this.description = this.classify();
    }
    return this.description;
  }

  readText(): string[] {
    if (TEXT_TYPES.includes(this.type)) {
      const d = this.describe();
      if (d.secure) {
        return d.hasSecureValue ? [UiOutlineFormat.SECURE_MASK] : [];
      }
      return [d.value ?? ''];
    }
    if (LABEL_TYPES.includes(this.type)) {
      return [attrString(this.attrs, 'content') ?? ''];
    }
    if (this.type === 'Button') {
      return [attrString(this.attrs, 'label') ?? ''];
    }
    return [];
  }

  readChildren(): boolean {
    return !TEXT_TYPES.includes(this.type);
  }

  isEnabled(): boolean {
    return attrBool(this.attrs, 'enabled') !== false;
  }

  /** 检查器属性 `focusable`。 */
  focusable(): boolean {
    return attrBool(this.attrs, 'focusable') === true;
  }

  /** 检查器属性 `focused`。 */
  isFocused(): boolean {
    return attrBool(this.attrs, 'focused') === true;
  }

  click(): boolean {
    const id = this.inspectorId;
    return id !== '' && this.scope.driver.clickById(id);
  }

  setText(text: string): boolean {
    return false;
  }

  selectOption(text: string): boolean {
    return false;
  }

  setRange(value: number): boolean {
    return false;
  }

  focus(): boolean {
    const id = this.inspectorId;
    return id !== '' && this.scope.driver.requestFocus(id);
  }

  imeAction(): boolean {
    return this.uniqueId >= 0 && this.scope.driver.dispatchKey(this.uniqueId, UiKey.ENTER);
  }

  canScroll(): boolean {
    return false;
  }

  scrollIntoView(): boolean {
    return false;
  }

  scrollPage(direction: UiScrollDirection): boolean {
    return false;
  }

  unsupportedReason(action: UiAction): string | undefined {
    if (action === 'scroll') {
      return NO_SCROLL_REASON;
    }
    const role = this.describe().role;
    if (action === 'fill' && (role === 'textbox' || role === 'searchbox')) {
      return NO_TEXT_REASON;
    }
    if (action === 'fill' && (role === 'combobox' || role === 'slider')) {
      return NO_SELECT_REASON;
    }
    return this.inspectorId === '' ? NO_ID_REASON : '派发点击失败（sendEventByKey 返回 false）';
  }

}

/** 窗口根或对话框覆盖层：作为 `window` / `dialog` 分组并承接按键。 */
export class ArkUiWindow extends ArkUiElement implements UiWindow {
  private readonly role: string;
  private readonly title: string;

  constructor(json: InspectorJson, scope: WindowScope, path: string, role: string, title: string) {
    super(json, scope, path, ArkUiWindow.windowRect(json), false);
    this.role = role;
    this.title = title;
  }

  private static windowRect(json: InspectorJson): UiRect | undefined {
    const w = Number(json.width ?? '');
    const h = Number(json.height ?? '');
    return Number.isFinite(w) && Number.isFinite(h) && w > 0 && h > 0 ? new UiRect(0, 0, w, h) : undefined;
  }

  isHidden(): boolean {
    return false;
  }

  describe(): UiDescription {
    const d: UiDescription = {
      kind: UiEntryKind.CONTAINER, role: this.role, name: this.title, states: [], required: false, secure: false,
      hasSecureValue: false, descend: true,
    };
    return d;
  }

  readText(): string[] {
    return [];
  }

  click(): boolean {
    return false;
  }

  focus(): boolean {
    return false;
  }

  sendKey(key: UiKey, target?: UiElement): boolean {
    const el = target instanceof ArkUiElement ? target as ArkUiElement : this as ArkUiElement;
    const id = el.uniqueId >= 0 ? el.uniqueId : this.uniqueId;
    return id >= 0 && this.scope.driver.dispatchKey(id, key);
  }

  /**
   * 焦点移到下一个 / 上一个可获焦组件（按检查器树序，循环）。
   * @why FocusController 没有"移到下一个"的接口，按树序 + `requestFocus(id)` 实现；只有设置了 id 的可获焦组件参与。
   */
  moveFocus(forward: boolean): boolean {
    const candidates: ArkUiElement[] = [];
    const walk = (e: ArkUiElement): void => {
      if (e.isHidden()) {
        return;
      }
      if (e !== this && e.inspectorId !== '' && e.isEnabled() && (e.focusable() || e.isFocused())) {
        candidates.push(e);
      }
      e.childElements().forEach((c: ArkUiElement) => walk(c));
    };
    walk(this);
    if (candidates.length === 0) {
      return false;
    }
    const current = candidates.findIndex((e: ArkUiElement) => e.isFocused());
    const next = current < 0 ? (forward ? 0 : candidates.length - 1) :
      (current + (forward ? 1 : candidates.length - 1)) % candidates.length;
    return candidates[next].focus();
  }

  dismiss(): boolean {
    return false;
  }
}

/** 一个窗口一次刷新的结果：窗口根、当前可见层（根或最上层对话框）。 */
class WindowState {
  readonly root: ArkUiWindow;
  readonly visible: ArkUiWindow;

  constructor(root: ArkUiWindow, visible: ArkUiWindow) {
    this.root = root;
    this.visible = visible;
  }
}

/**
 * ArkUI 平台：每次取窗口时重新读取各窗口的检查器树（不在后台观察界面）。
 * 同一窗口内最上层（树序最后）的对话框类覆盖层遮挡其余内容（spec/ui-fallback.md 4.4）。
 */
export class ArkUiPlatform implements UiPlatform {
  private readonly drivers: ArkUiWindowDriver[];
  private readonly settleDelayMs: number;
  private states: WindowState[] = [];
  private live: Set<string> = new Set<string>();

  constructor(drivers: ArkUiWindowDriver[], settleDelayMs: number = 50) {
    this.drivers = drivers;
    this.settleDelayMs = settleDelayMs;
  }

  addWindow(driver: ArkUiWindowDriver): void {
    if (!this.drivers.includes(driver)) {
      this.drivers.push(driver);
    }
  }

  removeWindow(driver: ArkUiWindowDriver): void {
    const i = this.drivers.indexOf(driver);
    if (i >= 0) {
      this.drivers.splice(i, 1);
    }
  }

  private static findOverlay(e: ArkUiElement): ArkUiElement | undefined {
    const found: ArkUiElement[] = [];
    const walk = (n: ArkUiElement): void => {
      if (DIALOG_TYPES.includes(n.type) && !n.isHidden()) {
        found.push(n);
        return;
      }
      n.childElements().forEach((c: ArkUiElement) => walk(c));
    };
    e.childElements().forEach((c: ArkUiElement) => walk(c));
    return found.length > 0 ? found[found.length - 1] : undefined;
  }

  private refresh(): void {
    const states: WindowState[] = [];
    const live = new Set<string>();
    this.drivers.forEach((driver: ArkUiWindowDriver, index: number) => {
      let json: InspectorJson;
      try {
        json = JSON.parse(driver.inspectorTree()) as InspectorJson;
      } catch (error) {
        throw UiFallbackInput.invalid(`读取窗口「${driver.title}」的组件树失败：${(error as Error).message}`);
      }
      const scope = new WindowScope(driver, index);
      const root = new ArkUiWindow(json, scope, 'root', 'window', driver.title);
      const overlay = ArkUiPlatform.findOverlay(root);
      let visible = root;
      if (overlay !== undefined) {
        const attrs: Record<string, Object> = overlay.json.$attrs ?? {};
        const role = overlay.type === 'AlertDialog' ? 'alertdialog' : 'dialog';
        visible = new ArkUiWindow(overlay.json, new WindowScope(driver, index), 'overlay', role,
          attrString(attrs, 'title') ?? '');
        visible.childElements();
      }
      // @why 存活集合按整棵树计算（含被遮挡的部分），遮挡层的键与根树中同一节点的键相同。
      const walkAll = (e: ArkUiElement): void => {
        live.add(e.identity());
        e.childElements().forEach((c: ArkUiElement) => walkAll(c));
      };
      walkAll(root);
      states.push(new WindowState(root, visible));
    });
    this.states = states;
    this.live = live;
  }

  windows(): UiWindow[] {
    this.refresh();
    return this.states.map((s: WindowState): UiWindow => s.visible);
  }

  allWindows(): UiWindow[] {
    this.refresh();
    return this.states.map((s: WindowState): UiWindow => s.root);
  }

  isLive(identity: string): boolean {
    return this.live.has(identity);
  }

  settle(): Promise<void> {
    return new Promise<void>((resolve: () => void) => {
      setTimeout(resolve, this.settleDelayMs);
    });
  }
}
