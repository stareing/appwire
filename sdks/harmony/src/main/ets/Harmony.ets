/**
 * 鸿蒙接入：加载原生模块、默认生命周期策略、跟随应用前后台、从 Want 中识别唤醒。
 *
 * ```ts
 * // AbilityStage.onCreate 或 EntryAbility.onCreate（主线程）
 * const mcp = HarmonyAppMcp.create(this.context, { appId: 'shop', appName: '示例商城' }, 'shopapp');
 * mcp.tool('cart.add', { description: '加入购物车', inputSchema: CART_ADD_SCHEMA, handler: addToCart });
 *
 * // EntryAbility
 * onCreate(want: Want): void { HarmonyAppMcp.handleWant(want); }
 * onNewWant(want: Want): void { HarmonyAppMcp.handleWant(want); }
 * ```
 *
 * @side-effect `create` 在 ApplicationContext 上注册 applicationStateChange 监听（`dispose()` 时注销），
 *   并把客户端记为进程级 `HarmonyAppMcp.current`（供意图执行器、`handleWant` 使用）。
 * @invariant 不申请长时任务、后台运行权限或 WakeLock：休眠后进程交给系统管理（spec/lifecycle.md 第 7 节）。
 */

import { ApplicationStateChangeCallback, common, Want } from '@kit.AbilityKit';
import { hilog } from '@kit.PerformanceAnalysisKit';
import native from 'libapp_mcp_harmony.so';
import type { ClientConfig, ClientEvent, NativeClient } from 'libapp_mcp_harmony.so';
import { AppMcp, NativeClientFactory } from './AppMcp';
import { AppMcpOptions, ConnectionState, LifecycleOptions, Logger, WakeDescriptor } from './Types';

/** hilog 的业务域与标签。 */
const LOG_DOMAIN: number = 0xA3C0;
const LOG_TAG: string = 'app-mcp';

/** Want.parameters 中携带唤醒令牌的键（`hdc shell aa start ... --ps app-mcp-wake <token>`）。 */
export const WAKE_PARAM: string = 'app-mcp-wake';

/** 唤醒 URI 的路径部分：`<scheme>://app-mcp/wake?token=<token>`。 */
export const WAKE_URI_PATH: string = 'app-mcp/wake';

export class HilogLogger implements Logger {
  debug(message: string): void {
    hilog.debug(LOG_DOMAIN, LOG_TAG, '%{public}s', message);
  }

  warn(message: string): void {
    hilog.warn(LOG_DOMAIN, LOG_TAG, '%{public}s', message);
  }

  error(message: string): void {
    hilog.error(LOG_DOMAIN, LOG_TAG, '%{public}s', message);
  }
}

/** 真实原生模块的客户端工厂。 */
export const nativeClientFactory: NativeClientFactory =
  (config: ClientConfig, listener: (event: ClientEvent) => void): NativeClient => new native.NativeClient(config, listener);

/** 不经 `HarmonyAppMcp.create`、直接创建客户端（不跟随前后台、不设默认生命周期）。 */
export function createAppMcp(options: AppMcpOptions): AppMcp {
  return new AppMcp(options, nativeClientFactory);
}

/** 从 Want 中提取唤醒参数：`want.uri`（唤醒 URI）与 `want.parameters['app-mcp-wake']`（令牌）。 */
export function wakeArgsFromWant(want: Want): string[] {
  const args: string[] = [];
  const uri = want.uri;
  if (uri !== undefined && uri !== '') {
    args.push(uri);
  }
  const params = want.parameters;
  if (params !== undefined) {
    const token = params[WAKE_PARAM];
    if (typeof token === 'string' && token !== '') {
      args.push(`app-mcp-wake:${token}`);
    }
  }
  return args;
}

export class HarmonyAppMcp {
  /** 最近一次 `create` 的客户端（已 dispose 时为 undefined）。 */
  static get current(): AppMcp | undefined {
    const client = HarmonyAppMcp.instance;
    return client !== undefined && !client.isDisposed ? client : undefined;
  }

  private static instance: AppMcp | undefined = undefined;

  /** 本 App 的唤醒描述：`uri` → `<scheme>://app-mcp/wake`（打开 URI 会把窗口带到前台）。 */
  static wakeDescriptor(scheme: string): WakeDescriptor {
    return { kind: 'uri', target: `${scheme}://${WAKE_URI_PATH}`, background: false };
  }

  /**
   * 鸿蒙默认生命周期：`on-demand`、`keep`、进入后台立即休眠（spec/lifecycle.md 第 13 节 B1「平台默认」、B4）。
   * 其余字段（合并窗口、hostAbsentRetries 等）取原生层默认值。
   * @compat `create` 收到 `options.lifecycle` 时整体使用它、不与本默认合并；只改个别字段时先取本默认再修改。
   */
  static defaultLifecycle(scheme?: string): LifecycleOptions {
    const lifecycle: LifecycleOptions = { mode: 'on-demand', residency: 'keep', sleepOnBackground: true };
    if (scheme !== undefined) {
      lifecycle.wake = HarmonyAppMcp.wakeDescriptor(scheme);
    }
    return lifecycle;
  }

  /**
   * 创建客户端并接入应用生命周期。
   *
   * @input context 任意 Context（UIAbility / AbilityStage / Application）；取其 ApplicationContext 监听前后台。
   * @input wakeScheme module.json5 `skills.uris` 中声明的 scheme；给出时填入唤醒描述，缺省由 Host 回退到清单 `launch`。
   * @error 原生模块加载失败或配置非法时抛出。
   */
  static create(context: common.Context, options: AppMcpOptions, wakeScheme?: string): AppMcp {
    const lifecycle: LifecycleOptions = options.lifecycle ?? HarmonyAppMcp.defaultLifecycle(wakeScheme);
    if (lifecycle.wake === undefined && wakeScheme !== undefined) {
      lifecycle.wake = HarmonyAppMcp.wakeDescriptor(wakeScheme);
    }
    const full: AppMcpOptions = {
      appId: options.appId,
      appName: options.appName,
      appVersion: options.appVersion,
      enabled: options.enabled,
      hostUrl: options.hostUrl,
      maxConcurrentCalls: options.maxConcurrentCalls,
      overview: options.overview,
      logger: options.logger ?? new HilogLogger(),
      instanceId: options.instanceId,
      instanceTitle: options.instanceTitle ?? options.appName,
      token: options.token,
      launchToken: options.launchToken,
      onPaired: options.onPaired,
      autoStart: options.autoStart,
      lifecycle: lifecycle,
      connectTimeoutMs: options.connectTimeoutMs,
      heartbeat: options.heartbeat,
      callDedup: options.callDedup,
      onIdleExit: options.onIdleExit,
    };
    const client = createAppMcp(full);
    HarmonyAppMcp.instance = client;
    if (options.enabled !== false) {
      let appContext: common.ApplicationContext | undefined = undefined;
      try {
        appContext = context.getApplicationContext();
      } catch (error) {
        hilog.warn(LOG_DOMAIN, LOG_TAG, '取不到 ApplicationContext，不跟随前后台：%{public}s', (error as Error).message);
      }
      if (appContext !== undefined) {
        HarmonyAppMcp.trackVisibility(appContext, client, lifecycle);
      }
    }
    return client;
  }

  /**
   * 把 UIAbility 收到的 Want（`onCreate` / `onNewWant`）交给客户端；是本 SDK 的唤醒时回连并返回 true。
   * `client` 缺省为 {@link HarmonyAppMcp.current}。
   */
  static handleWant(want: Want, client?: AppMcp): boolean {
    const target = client ?? HarmonyAppMcp.current;
    if (target === undefined) {
      return false;
    }
    const args = wakeArgsFromWant(want);
    return args.length > 0 && target.handleWake(args);
  }

  private static trackVisibility(appContext: common.ApplicationContext, client: AppMcp,
    lifecycle: LifecycleOptions): void {
    const callback: ApplicationStateChangeCallback = {
      onApplicationForeground: (): void => {
        client.setVisibility('visible', true);
        // @why 休眠中回到前台时回连（已连接时只重新计时）；`on-demand` 启动后处于 dormant，
        //   在 onCreate 中 create 时首次进入前台也经此连接。

        if (lifecycle.mode !== undefined && lifecycle.mode !== 'persistent') {
          client.wake('visible');
        }
      },
      onApplicationBackground: (): void => {
        client.setVisibility('hidden', false);
      },
    };
    try {
      appContext.on('applicationStateChange', callback);
    } catch (error) {
      // @why 只在非主线程调用时失败（applicationStateChange 只能在主线程注册）；不影响工具调用。
      hilog.warn(LOG_DOMAIN, LOG_TAG, '无法跟随应用前后台：%{public}s', (error as Error).message);
      return;
    }
    client.onStateChange((state: ConnectionState): void => {
      if (state.status === 'stopped') {
        try {
          appContext.off('applicationStateChange', callback);
        } catch (error) {
          hilog.warn(LOG_DOMAIN, LOG_TAG, '注销前后台监听失败：%{public}s', (error as Error).message);
        }
      }
    });
  }
}
