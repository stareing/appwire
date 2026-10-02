/**
 * 鸿蒙 ArkUI 进程内控件兜底（spec/ui-fallback.md，第 4c 项 H）：开发者显式开启后注册
 * `<prefix>.outline` / `click` / `fill` / `press` / `scroll` / `read`，经 ArkUI 检查器树与按组件 id / uniqueId 派发的事件
 * 操作控件，不截图、不按坐标。
 *
 * ```ts
 * // EntryAbility.onWindowStageCreate 中 loadContent 完成后（主线程）
 * const ui = windowStage.getMainWindowSync().getUIContext();
 * fallback = HarmonyUiFallback.enable(mcp, [new UIContextDriver(ui, '示例商城')],
 *   { applicationContext: this.context.getApplicationContext() });
 * // 页面中：Button('清空').id('clear').customProperty(MCP_DECLARED_PROPERTY, 'cart.clear')
 * ```
 *
 * - 工具以 `surface: 'view'` 注册，只在应用处于前台时启用（`applicationStateChange`，或 `setVisible` 手动指定）；
 * - 每个动作重新读取检查器树、按引用取控件，核对仍在界面上、可见、启用；
 * - 密码类输入框只显示 `••••`，拒绝填写与按键。
 *
 * @invariant 只在 UI 线程（创建客户端的 ArkTS 线程）上调用；不常驻任何定时器，只在调用时读取组件树。
 */

import { ApplicationStateChangeCallback, common } from '@kit.AbilityKit';
import { ToolCallError } from '../Errors';
import { Registrar, Scope, ToolContext, ToolHandle } from '../Types';
import { ArkUiPlatform, ArkUiWindowDriver } from './ArkUiElements';
import { UiInspector } from './UiInspector';
import { UiArgs, UiFallbackInput, UiFallbackSchemas } from './UiFallbackInput';
import { UiOutlineFormat } from './UiOutlineFormat';

/** {@link HarmonyUiFallback.enable} 的选项。 */
export interface UiFallbackOptions {
  /** 工具名前缀，缺省 `ui`。 */
  prefix?: string;
  /** `outline` 缺省列出的控件数，缺省 60。 */
  maxItems?: number;
  /** 操作后再等的时长（毫秒），缺省 50。 */
  settleDelayMs?: number;
  /** 开启时是否已有可见窗口，缺省 true（在 onWindowStageCreate / 页面中开启）。 */
  visible?: boolean;
  /** 给出时跟随应用前后台（applicationStateChange）启用 / 禁用工具；`close()` 时注销监听。 */
  applicationContext?: common.ApplicationContext;
}

type UiRun = (args: UiArgs) => Object | Promise<Object>;

export class HarmonyUiFallback {
  readonly inspector: UiInspector;
  readonly platform: ArkUiPlatform;
  private readonly scope: Scope;
  private readonly tools: ToolHandle[] = [];
  private readonly appContext: common.ApplicationContext | undefined;
  private callback: ApplicationStateChangeCallback | undefined = undefined;
  private visible: boolean = false;
  private closed: boolean = false;

  private constructor(scope: Scope, inspector: UiInspector, platform: ArkUiPlatform,
    appContext: common.ApplicationContext | undefined) {
    this.scope = scope;
    this.inspector = inspector;
    this.platform = platform;
    this.appContext = appContext;
  }

  /** 兜底工具当前是否启用。 */
  get enabled(): boolean {
    return !this.closed && this.visible;
  }

  /** 应用是否有可见窗口（前台）；变化时启用 / 禁用全部兜底工具，隐藏时丢弃引用。 */
  setVisible(visible: boolean): void {
    if (this.closed || visible === this.visible) {
      return;
    }
    this.visible = visible;
    this.tools.forEach((t: ToolHandle) => t.update({ enabled: visible }));
    if (!visible) {
      this.inspector.clear();
    }
  }

  /** 增加窗口（子窗口的 UIContext）。 */
  addWindow(driver: ArkUiWindowDriver): void {
    this.platform.addWindow(driver);
  }

  removeWindow(driver: ArkUiWindowDriver): void {
    this.platform.removeWindow(driver);
  }

  private add(prefix: string, name: string, title: string, description: string, schema: string, readOnly: boolean,
    run: UiRun): void {
    this.tools.push(this.scope.tool<UiArgs, Object>(`${prefix}.${name}`, {
      description: description,
      title: title,
      inputSchema: schema,
      risk: readOnly ? 'read' : 'write',
      annotations: { title: title, readOnlyHint: readOnly },
      enabled: false,
      surface: 'view',
      handler: (args: UiArgs, context: ToolContext): Object | Promise<Object> => {
        if (!this.enabled) {
          throw new ToolCallError('TOOL_DISABLED', '应用当前没有可见窗口，兜底工具不可用');
        }
        return run(args ?? {});
      },
    }));
  }

  private register(prefix: string): void {
    const ui = this.inspector;
    this.add(prefix, 'outline', '界面控件大纲',
      '兜底能力：列出当前窗口可见的可交互控件（按钮、输入框、复选框等），每行一个，带引用 eN，按窗口 / 对话框 / 分组归类。' +
        '应用已有对应的业务工具或标注 [已声明：…] 时请优先使用那些工具。引用用于 click / fill / press / scroll / read。',
      UiFallbackSchemas.outline(ui.maxItems), true,
      (a: UiArgs): Object => ui.outline(UiFallbackInput.str(a, 'query'), UiFallbackInput.ref(a, 'within', false), UiFallbackInput.int(a, 'limit')));
    this.add(prefix, 'click', '点击控件',
      `兜底能力：激活 ${prefix}.outline 中的控件（点击按钮、切换复选框、选中选项），返回界面变化摘要。`,
      UiFallbackSchemas.REF_ONLY, false, (a: UiArgs): Promise<Object> => ui.click(UiFallbackInput.requiredRef(a, 'ref')));
    this.add(prefix, 'fill', '填写控件',
      '兜底能力：复选框 / 开关 / 单选框传 true / false。ArkUI 不支持从组件外写入文本、选择下拉项或设置滑块；密码类控件不支持。返回界面变化摘要。',
      UiFallbackSchemas.FILL, false,
      (a: UiArgs): Promise<Object> => ui.fill(UiFallbackInput.requiredRef(a, 'ref'), UiFallbackInput.value(a)));
    this.add(prefix, 'press', '按键',
      '兜底能力：按键（ref 缺省为当前焦点控件）：Enter（提交文本框 / 激活）、Escape（关闭对话框）、Tab / Shift+Tab（移动焦点）、Space（激活）。' +
        '输入文本请用 fill。返回界面变化摘要。',
      UiFallbackSchemas.PRESS, false, (a: UiArgs): Promise<Object> => {
        const key = UiFallbackInput.str(a, 'key');
        if (key === undefined) {
          throw UiFallbackInput.invalid('缺少参数 key');
        }
        return ui.press(UiFallbackInput.ref(a, 'ref', false), key);
      });
    this.add(prefix, 'scroll', '滚动',
      '兜底能力：无 direction 时把控件滚动到可见；direction 为 up / down / left / right 时滚动该控件所在的滚动区一页。ArkUI 下不支持（返回 unsupported）。',
      UiFallbackSchemas.SCROLL, false,
      (a: UiArgs): Promise<Object> => ui.scroll(UiFallbackInput.requiredRef(a, 'ref'), UiFallbackInput.str(a, 'direction')));
    this.add(prefix, 'read', '读取控件文本',
      `兜底能力：读取控件的可见文本（折叠空白，默认最多 ${UiOutlineFormat.READ_DEFAULT} 字）；ref 缺省为全部窗口。`,
      UiFallbackSchemas.READ, true,
      (a: UiArgs): Object => ui.read(UiFallbackInput.ref(a, 'ref', false), UiFallbackInput.int(a, 'maxChars')));
  }

  private trackForeground(): void {
    const appContext = this.appContext;
    if (appContext === undefined) {
      return;
    }
    const callback: ApplicationStateChangeCallback = {
      onApplicationForeground: (): void => this.setVisible(true),
      onApplicationBackground: (): void => this.setVisible(false),
    };
    try {
      appContext.on('applicationStateChange', callback);
      this.callback = callback;
    } catch (error) {
      // @why 只能在主线程注册；失败时保持 options.visible 的初值，由调用方经 setVisible 维护。
      this.callback = undefined;
    }
  }

  /** 注销全部兜底工具与前后台监听（幂等）。 */
  close(): void {
    if (this.closed) {
      return;
    }
    this.closed = true;
    const appContext = this.appContext;
    const callback = this.callback;
    if (appContext !== undefined && callback !== undefined) {
      try {
        appContext.off('applicationStateChange', callback);
      } catch (error) {
        // @why 只在非主线程调用时失败；工具已随 scope 注销。
      }
    }
    this.inspector.clear();
    this.tools.length = 0;
    this.scope.dispose();
  }

  /**
   * 在 registrar（客户端或作用域）下建子作用域 `<prefix>-fallback` 并注册兜底工具。必须在 UI 线程调用。
   * @input windows 要操作的窗口（主窗口与子窗口的 UIContext 驱动），之后可用 addWindow / removeWindow 调整
   */
  static enable(registrar: Registrar, windows: ArkUiWindowDriver[], options?: UiFallbackOptions): HarmonyUiFallback {
    const prefix = options?.prefix ?? 'ui';
    const platform = new ArkUiPlatform(windows.slice(), options?.settleDelayMs ?? 50);
    const inspector = new UiInspector(platform, prefix, options?.maxItems ?? 60);
    const fallback = new HarmonyUiFallback(registrar.scope(`${prefix}-fallback`), inspector, platform,
      options?.applicationContext);
    try {
      fallback.register(prefix);
      fallback.trackForeground();
      fallback.setVisible(options?.visible ?? true);
      return fallback;
    } catch (error) {
      fallback.close();
      throw error as Error;
    }
  }
}
