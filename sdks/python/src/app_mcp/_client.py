"""app-mcp Python SDK 的惯用封装（建在 uniffi 生成的 ``app_mcp_uniffi`` 之上）。

线程模型
--------
原生运行时在自己的**分发线程**上同步调用 handler 回调，回调必须尽快返回。本模块在回调里只做
参数解析和登记，然后：

- 同步函数：交给 ``dispatcher``（默认内部线程池；可传入切到 Qt / Tk 主线程的调度函数）执行；
- ``async def``：用 ``asyncio.run_coroutine_threadsafe`` 投递到事件循环（默认内部后台循环线程，
  可用 ``loop=`` 指定应用自己的循环，如 qasync 的 Qt 循环）。

函数执行完后从所在线程调用 ``Call.complete`` / ``Call.fail``。
调度后超过 ``dispatch_timeout`` 秒仍未开始执行（UI 线程卡住）时以 ``APP_NOT_RESPONDING`` 失败。
"""

from __future__ import annotations

import asyncio
import concurrent.futures
import inspect
import json
import threading
from collections.abc import Callable
from collections.abc import Mapping
from collections.abc import Sequence
from contextlib import AbstractContextManager
from typing import Any, Literal

from . import app_mcp_uniffi as ffi
from ._busy import BusyPolicyLike, _BusyState, _busy_policy
from ._lifecycle import LifecyclePolicy
from ._schema import ArgumentBinder, to_jsonable
# @compat 以下名称已拆分到子模块，经本模块再导出以保持原导入路径
from ._client_convert import (  # noqa: F401
    logger, Dispatcher, F, RiskLike, ActivationLike, SurfaceLike, ToolAnnotationsLike, ContentAnnotationsLike,
    ResultStatusLike, _Unset, _UNSET, _RISKS, _ACTIVATIONS, _VISIBILITIES, _MODES, _RESIDENCIES, _WAKE_KINDS,
    _HEARTBEATS, _WAKE_REASONS, _RESULT_STATUSES, _SURFACES, _AUDIENCES, _TOOL_ANNOTATION_KEYS,
    _SLEEP_REASONS, ERROR_KINDS, _ms, _lifecycle_to_ffi, _enum_arg, _risk, _tool_annotations,
    _content_annotations, _schema_json, _surface, _activation, _implements, CacheLike, _cache,
    DeprecationLike, _deprecation,
)
from ._client_types import (  # noqa: F401
    CallDedup, ToolCallError, UserActionReason, Hold, ToolResult, ToolContext, _CancelListener,
    NavigationDenied,
)
from ._client_adapters import (  # noqa: F401
    _error_of, _complete_ok, _complete_err, _Gate, _Registration, _ToolAdapter, _ResourceAdapter,
    NavigateFunction, _finish_navigate, _user_action_fields, _fail_navigate, _NavigationAdapter,
    _ClientListener,
)
from ._client_handles import (  # noqa: F401
    ToolHandle, _replace_spec, ResourceHandle,
)

__all__ = [
    "AppMcp",
    "Scope",
    "ToolHandle",
    "ResourceHandle",
    "ToolContext",
    "ToolResult",
    "ToolCallError",
    "UserActionReason",
    "Dispatcher",
    "Hold",
    "CallDedup",
]

# ---------------------------------------------------------------------------
# 注册入口（客户端与 Scope 共用）
# ---------------------------------------------------------------------------


class _Registrar:
    _owner: AppMcp

    def _raw(self) -> ffi.AppMcpClient | ffi.Scope:
        raise NotImplementedError

    def add_tool(
        self,
        fn: Callable[..., Any],
        name: str | None = None,
        description: str | None = None,
        *,
        input_schema: dict[str, Any] | str | None = None,
        risk: RiskLike | None = None,
        activation: ActivationLike | None = None,
        title: str | None = None,
        enabled: bool = True,
        annotations: ToolAnnotationsLike | None = None,
        output_schema: dict[str, Any] | str | None = None,
        surface: SurfaceLike | None = None,
        page: str | None = None,
        background_tool: str | None = None,
        concurrency: int = 0,
        exclusive: str | None = None,
        implements: Sequence[str] = (),
        cache: CacheLike | None = None,
        deprecated: DeprecationLike | None = None,
    ) -> ToolHandle:
        """注册函数为工具，返回句柄。``input_schema`` 缺省时从函数签名生成。

        ``risk`` 为旧写法，优先用 ``annotations``：标准 MCP 工具注解（``{"read_only_hint": True}`` 或
        ``ToolAnnotations``），原样转发给 Agent；为空时 Hub 按 ``risk`` 推导。``output_schema`` 为结果的
        JSON Schema（字典或 JSON 文本，MCP ``outputSchema``）。

        ``surface``：``"app"``（缺省，不依赖界面）/ ``"view"``（只在所在界面可见且在最上层时启用，spec/protocol.md 3.4；
        Qt 可用 :func:`app_mcp.qt.bind_view_tool` 按 show / hide 切换）；``page``：所在页面名，Hub 在该工具未注册时
        据此导航（:meth:`AppMcp.set_navigation_handler`）；``background_tool``：后台替身（只对 ``"view"`` 工具有意义），
        同 App 内一个 ``"app"`` 工具的名称，App 在后台、本工具不可调用时 Hub 改调该工具（spec/protocol.md 3.4「后台与前台」）。

        ``concurrency``：本工具同时执行的调用上限（``0`` 缺省 = 不单独限制，只受 ``max_concurrent_calls`` 约束）；
        ``exclusive``：互斥组名（命名规则同工具名），同组工具同一时刻最多执行一个调用。两者只在 SDK 内排队生效，
        不发给 Host（spec/protocol.md 5.3）。

        ``implements``：本工具实现的标准意图（spec/intents.md，每项 ``"<动词>@<主版本>"``，如 ``["message.send@1"]``，
        单个字符串视为一项），Agent 经 ``apps.intents`` 按动词找到实现者；格式不合法时抛 ``AppMcpError.InvalidName``，
        未知动词或缺少词表必填参数只给出警告。

        ``cache``：结果缓存声明（spec/protocol.md 3.6），``{"ttl_ms": 60000, "scope": "shared"}``、``ttl_ms`` 整数或
        ``CachePolicy``；``ttl_ms`` 内相同参数的调用 Hub 可直接返回上次结果、不调用本函数。只对生效注解只读的工具生效；
        ``scope`` 缺省 ``"private"``（按调用方隔离），``"shared"`` 只用于与调用方无关的数据。``ttl_ms`` 越界时抛
        ``AppMcpError.InvalidConfig``。

        ``deprecated``：弃用声明（spec/protocol.md 3.7），``{"message": "改用 x.new", "replacement": "x.new",
        "until": "2027-06-30"}``、纯字符串（即 ``message``）或 ``Deprecation``；弃用的工具照常列出与调用，Agent 看到
        弃用提示。``message`` 为 1..=500 个字符，``replacement`` 为同 App 内另一工具的局部名，``until`` 为
        ``YYYY-MM-DD``（只作提示）；不合法时抛 ``AppMcpError.InvalidConfig``。
        """
        binder = ArgumentBinder(fn, ToolContext)
        if input_schema is None:
            schema = binder.schema()
        else:
            schema = json.loads(input_schema) if isinstance(input_schema, str) else input_schema
        spec = ffi.ToolSpec(
            name=name or fn.__name__,
            description=description if description is not None else inspect.getdoc(fn) or "",
            input_schema_json=json.dumps(schema) if schema is not None else None,
            risk=_risk(risk),
            activation=_activation(activation),
            title=title,
            enabled=enabled,
            annotations=_tool_annotations(annotations),
            output_schema_json=_schema_json(output_schema),
            surface=_surface(surface),
            page=page,
            background_tool=background_tool,
            concurrency=concurrency,
            exclusive=exclusive,
            implements=_implements(implements),
            cache=_cache(cache),
            deprecated=_deprecation(deprecated),
        )
        adapter = _ToolAdapter(_Registration(self._owner, fn, binder))
        return ToolHandle(self._raw().register_tool(spec, adapter), spec)

    def tool(
        self,
        name: str | None = None,
        description: str | None = None,
        *,
        input_schema: dict[str, Any] | str | None = None,
        risk: RiskLike | None = None,
        activation: ActivationLike | None = None,
        title: str | None = None,
        enabled: bool = True,
        annotations: ToolAnnotationsLike | None = None,
        output_schema: dict[str, Any] | str | None = None,
        surface: SurfaceLike | None = None,
        page: str | None = None,
        background_tool: str | None = None,
        concurrency: int = 0,
        exclusive: str | None = None,
        implements: Sequence[str] = (),
        cache: CacheLike | None = None,
        deprecated: DeprecationLike | None = None,
    ) -> Callable[[F], F]:
        """装饰器形式的 :meth:`add_tool`。返回原函数；句柄可用 ``client.tools[name]`` 取得。"""

        def decorator(fn: F) -> F:
            handle = self.add_tool(
                fn,
                name,
                description,
                input_schema=input_schema,
                risk=risk,
                activation=activation,
                title=title,
                enabled=enabled,
                annotations=annotations,
                output_schema=output_schema,
                surface=surface,
                page=page,
                background_tool=background_tool,
                concurrency=concurrency,
                exclusive=exclusive,
                implements=implements,
                cache=cache,
                deprecated=deprecated,
            )
            self._owner.tools[handle.name] = handle
            return fn

        return decorator

    def add_resource(
        self,
        fn: Callable[[], Any],
        name: str | None = None,
        description: str | None = None,
        *,
        mime_type: str | None = None,
        realtime: bool = False,
        annotations: ContentAnnotationsLike | None = None,
        cache: CacheLike | None = None,
    ) -> ResourceHandle:
        """注册资源读取函数（无参数，返回可 JSON 序列化的内容）。

        ``realtime``：需实时推送（spec/lifecycle.md 第 13 节 B3）——被订阅时阻止休眠、休眠中变化时回连推送；
        默认 ``False``：订阅不阻止休眠，变化在下次连接时补发。
        ``annotations``：资源内容的标注（``{"audience": ["user"], "priority": 0.5}``），Hub 放到 MCP ``resources/list``
        的资源注解上。读取函数抛出 :class:`ToolCallError`（含 :meth:`ToolCallError.user_action_required`）时，
        类别与详情原样交给 Host。``cache``：读取结果缓存声明，形式与 :meth:`add_tool` 相同。
        """
        spec = ffi.ResourceSpec(
            name=name or fn.__name__,
            description=description if description is not None else inspect.getdoc(fn) or "",
            mime_type=mime_type,
            realtime=realtime,
            annotations=_content_annotations(annotations),
            cache=_cache(cache),
        )
        adapter = _ResourceAdapter(_Registration(self._owner, fn, None))
        return ResourceHandle(self._raw().register_resource(spec, adapter))

    def resource(
        self,
        name: str | None = None,
        description: str | None = None,
        *,
        mime_type: str | None = None,
        realtime: bool = False,
        annotations: ContentAnnotationsLike | None = None,
        cache: CacheLike | None = None,
    ) -> Callable[[F], F]:
        """装饰器形式的 :meth:`add_resource`。句柄可用 ``client.resources[name]`` 取得。"""

        def decorator(fn: F) -> F:
            handle = self.add_resource(
                fn, name, description, mime_type=mime_type, realtime=realtime, annotations=annotations, cache=cache
            )
            self._owner.resources[handle.name] = handle
            return fn

        return decorator

    def scope(self, name: str) -> Scope:
        """创建子作用域。``dispose()``（或 ``with`` 结束）时注销其下全部工具与资源。"""
        return Scope(self._owner, self._raw().create_scope(name))


class Scope(_Registrar):
    def __init__(self, owner: AppMcp, inner: ffi.Scope) -> None:
        self._owner = owner
        self._inner = inner

    def _raw(self) -> ffi.Scope:
        return self._inner

    def dispose(self) -> None:
        self._inner.dispose()

    def __enter__(self) -> Scope:
        return self

    def __exit__(self, *exc: object) -> None:
        self.dispose()


# ---------------------------------------------------------------------------
# 客户端
# ---------------------------------------------------------------------------


class AppMcp(_Registrar):
    """app-mcp 客户端。

    >>> client = AppMcp(app_id="shop", app_name="Shop",
    ...                 overview=AppOverview(summary="网店：浏览商品、管理购物车"))
    >>> @client.tool("cart.add", description="加入购物车", risk="write")
    ... def add(sku: str, qty: int = 1) -> dict:
    ...     return {"ok": True}
    >>> client.start()

    ``lifecycle`` 为空时 ``persistent``（不休眠）；``heartbeat``：``"auto"``（默认，按传输：本地 IPC / 桌面回环不发）/
    ``"always"`` / ``"off"``（spec/lifecycle.md 第 11 节 A3）；``call_dedup``：调用去重（:class:`CallDedup`，
    缺省 300 秒、64 条，``CallDedup.OFF`` 关闭）；``navigate_in_background``：后台时是否仍把导航交给导航回调
    （:meth:`set_navigate_in_background`），``None`` 取平台默认（桌面为 ``True``）。
    ``max_queued_calls``：排队中的调用上限，满后新到的调用以 ``RATE_LIMITED``（``scope = "queue"``）拒绝；
    ``None`` 取核心缺省 64，``0`` 不限（spec/protocol.md 5.3）。
    ``busy_policy``：用户正在操作（:meth:`set_busy`）期间写调用的处理方式，``"reject"``（``None`` 时的缺省，以
    ``RATE_LIMITED``、``data.scope == "busy"`` 拒绝）或 ``"queue"``（排队，停手后按序执行）；运行时可用
    :meth:`set_busy_policy` 修改（spec/protocol.md 5.3「用户正在操作」）。

    ``register_name``：按名寻址（spec/naming.md）——``start()`` 后在系统名字服务登记，Hub 按名拨入、进程未运行时由系统
    激活（Linux：D-Bus 会话总线名 ``dev.appmcp.App.<app_id>``；Windows：命名管道；都需先 ``app-mcp-host app install``
    登记），通常与 ``LifecyclePolicy(mode=LifecycleMode.ON_DEMAND)`` 同用；本平台不支持（macOS 等）时记一条日志、其余照常。
    ``name_instance``：另登记实例名（``[a-z][a-z0-9-]{0,31}``，不能是 ``"default"``），供 ``appmcp://<app_id>/<instance>``
    寻址；不合法时构造抛出 ``AppMcpError.InvalidConfig``。
    """

    def __init__(
        self,
        app_id: str,
        app_name: str,
        *,
        host_url: str | None = None,
        instance_id: str | None = None,
        app_version: str | None = None,
        instance_title: str | None = None,
        token: str | None = None,
        launch_token: str | None = None,
        max_concurrent_calls: int = 1,
        max_queued_calls: int | None = None,
        busy_policy: BusyPolicyLike | None = None,
        overview: ffi.AppOverview | str | None = None,
        dispatcher: Dispatcher | None = None,
        loop: asyncio.AbstractEventLoop | None = None,
        dispatch_timeout: float | None = 10.0,
        on_state: Callable[[ffi.StateInfo], None] | None = None,
        on_paired: Callable[[str], None] | None = None,
        lifecycle: LifecyclePolicy | None = None,
        connect_timeout: float | None = None,
        on_idle_exit: Callable[[], None] | None = None,
        heartbeat: Literal["auto", "always", "off"] | ffi.HeartbeatMode = "auto",
        call_dedup: CallDedup | None = None,
        navigate_in_background: bool | None = None,
        register_name: bool = False,
        name_instance: str | None = None,
    ) -> None:
        self._owner = self
        self._on_idle_exit = on_idle_exit
        self.lifecycle = lifecycle
        self.tools: dict[str, ToolHandle] = {}
        self.resources: dict[str, ResourceHandle] = {}
        self._on_state = on_state
        self._on_paired = on_paired
        self._dispatch_timeout = dispatch_timeout
        self._state_cond = threading.Condition()
        self._closed = False

        self._executor: concurrent.futures.ThreadPoolExecutor | None = None
        if dispatcher is None:
            self._executor = concurrent.futures.ThreadPoolExecutor(
                max_workers=max(4, max_concurrent_calls), thread_name_prefix="app-mcp"
            )
            executor = self._executor

            def default_dispatch(fn: Callable[[], None]) -> None:
                executor.submit(fn)

            dispatcher = default_dispatch
        self._dispatcher: Dispatcher = dispatcher

        self._loop = loop
        self._own_loop_thread: threading.Thread | None = None
        self._loop_lock = threading.Lock()

        config = ffi.ClientConfig(
            app_id=app_id,
            app_name=app_name,
            instance_id=instance_id,
            host_url=host_url,
            app_version=app_version,
            instance_title=instance_title,
            token=token,
            launch_token=launch_token,
            max_concurrent_calls=max_concurrent_calls,
            max_queued_calls=max_queued_calls,
            busy_policy=None if busy_policy is None else _busy_policy(busy_policy),
            overview=ffi.AppOverview(summary=overview) if isinstance(overview, str) else overview,
            lifecycle=None if lifecycle is None else _lifecycle_to_ffi(lifecycle),
            connect_timeout_ms=None if connect_timeout is None else max(1, _ms(connect_timeout)),
            heartbeat=_enum_arg(heartbeat, _HEARTBEATS, "心跳策略"),
            call_dedup=None if call_dedup is None else call_dedup._ffi(),
            register_name=register_name,
            name_instance=name_instance,
        )
        self._inner = ffi.AppMcpClient(config, _ClientListener(self))
        self._busy = _BusyState(self._inner.set_busy)
        if navigate_in_background is not None:
            self._inner.set_navigate_in_background(navigate_in_background)
        self._state: ffi.StateInfo = self._inner.state()

    def _raw(self) -> ffi.AppMcpClient:
        return self._inner

    # -- 生命周期 ------------------------------------------------------------

    def start(self) -> AppMcp:
        """开始连接 Host（重复调用无效果）。"""
        self._inner.start()
        return self

    def stop(self) -> None:
        """停止：取消所有进行中的调用、断开连接、不再重连。"""
        self._inner.stop()

    def handle_wake(self, argv: str | Sequence[str]) -> bool:
        """处理操作系统激活参数：命令行参数列表（如 ``sys.argv[1:]``）、单个参数或 URL。

        识别 ``app-mcp-wake:<token>``、``<scheme>://app-mcp/wake?token=``、``#app-mcp-wake=<token>``；
        是本 SDK 的唤醒时发起回连并返回 ``True``。可以在 ``start()`` 之前调用（冷启动唤醒）。
        """
        if isinstance(argv, str):
            return self._inner.handle_wake(argv)
        for arg in argv:
            if ffi.parse_wake_token(arg) is not None:
                return self._inner.handle_wake(arg)
        return False

    def wake(self, reason: str | ffi.WakeReason | None = None) -> bool:
        """App 主动回连（如用户打开了相关界面）；``reason="visible"`` 表示窗口重新可见。返回是否发起了回连。"""
        if reason is None:
            return self._inner.wake()
        return self._inner.wake_with_reason(_enum_arg(reason, _WAKE_REASONS, "回连原因"))

    def connect_now(self) -> bool:
        """``on-demand`` 模式下主动连接；尚未 ``start()`` 时等同于 ``start()``。"""
        return self._inner.connect_now()

    def sleep(self, reason: str | ffi.SleepReason | None = None) -> bool:
        """App 主动请求休眠（默认原因 ``app``；进入后台时可用 ``"background"``）。返回是否有效果。"""
        if reason is None:
            return self._inner.sleep()
        return self._inner.sleep_with_reason(_enum_arg(reason, _SLEEP_REASONS, "休眠原因"))

    def hold(self) -> Hold:
        """临时阻止自动休眠，直到返回的 :class:`Hold` 被释放（``with client.hold(): ...``）。"""
        return Hold(self._inner.hold())

    @property
    def tools_hash(self) -> str:
        """当前工具与资源定义的摘要（spec/lifecycle.md 第 6 节）。"""
        return self._inner.tools_hash()

    def _idle_exit(self) -> None:
        cb = self._on_idle_exit
        if cb is None:
            return

        def run() -> None:
            try:
                cb()
            except Exception:
                logger.exception("on_idle_exit 回调抛出异常")

        try:
            self._dispatcher(run)
        except Exception:  # 调度器已关闭：直接在分发线程上调用
            run()

    def close(self) -> None:
        """停止并释放内部线程池 / 事件循环。"""
        if self._closed:
            return
        self._closed = True
        self._inner.stop()
        if self._executor is not None:
            self._executor.shutdown(wait=False, cancel_futures=True)
        if self._own_loop_thread is not None and self._loop is not None:
            self._loop.call_soon_threadsafe(self._loop.stop)
            self._own_loop_thread.join(timeout=2)

    def __enter__(self) -> AppMcp:
        return self.start()

    def __exit__(self, *exc: object) -> None:
        self.close()

    # -- 状态 ----------------------------------------------------------------

    @property
    def instance_id(self) -> str:
        return self._inner.instance_id()

    @property
    def token(self) -> str | None:
        return self._inner.token()

    @property
    def connection_id(self) -> str | None:
        """Host 为当前连接分配的连接 ID（spec/protocol.md 10.3），与 Host 日志中的 cid 对应；未连接时为 None。"""
        return self._inner.connection_id()

    @property
    def state(self) -> ffi.StateInfo:
        return self._inner.state()

    def wait_for_state(self, status: ffi.StateStatus, timeout: float | None = None) -> bool:
        """阻塞直到进入指定状态，返回是否在超时前达到。"""
        with self._state_cond:
            return self._state_cond.wait_for(lambda: self._inner.state().status == status, timeout)

    def set_navigation_handler(self, fn: NavigateFunction | None) -> None:
        """设置导航回调（Host 的 ``app/navigate``，spec/protocol.md 3.4）；``None`` 清除（之后的导航请求以
        ``NAVIGATION_FAILED`` 回复）。

        ``fn(page, params)`` 切换到页面后正常返回即完成；抛 :class:`NavigationDenied` 拒绝；抛
        :meth:`ToolCallError.user_action_required` 按需要用户操作回复（如 App 在后台、已发通知请用户点开：
        ``reason=UserActionReason.FOREGROUND``、``uri`` 为该页面的 App 内入口）；其他异常按失败回复。
        同步函数经 ``dispatcher`` 执行（传 :func:`~app_mcp.qt_dispatcher` / :func:`~app_mcp.tk_dispatcher` 即在 UI 线程），
        ``async`` 函数在事件循环上执行；``dispatch_timeout`` 内未开始执行时以失败回复。

        能力在握手时声明：建议在 :meth:`start` 之前设置；连接后才设置的回调在下次连接（回连 / 唤醒）时生效。
        """
        self._inner.set_navigation_handler(None if fn is None else _NavigationAdapter(self, fn))

    def on_navigate(self, fn: F) -> F:
        """装饰器形式的 :meth:`set_navigation_handler`。"""
        self.set_navigation_handler(fn)
        return fn

    def set_navigate_in_background(self, enabled: bool) -> None:
        """后台时是否仍把导航请求交给导航回调（spec/protocol.md 3.4「后台与前台」），随时生效。为 ``False`` 时
        App 不可见（``hidden`` / ``frozen``）收到的导航立即以 ``USER_ACTION_REQUIRED``（reason ``foreground``）回复，
        不调用回调。缺省取平台默认（桌面为 ``True``）。"""
        self._inner.set_navigate_in_background(enabled)

    # -- 用户正在操作（spec/protocol.md 5.3） ---------------------------------

    def set_busy(self, busy: bool) -> None:
        """声明用户正在 / 不再在 App 内操作（何时算由 App 决定，如编辑框获得焦点、拖拽中）。期间写调用（生效注解不是
        ``readOnlyHint: true`` 的工具）按 ``busy_policy`` 拒绝或排队；只读调用不受影响。状态只在 SDK 内，不发给 Host。

        与 :meth:`busy` 作用域合并：生效值为「本开关 ∨ 仍有作用域未退出」，``set_busy(False)`` 不结束进行中的作用域。
        """
        self._busy.set(busy)

    def busy(self) -> AbstractContextManager[None]:
        """作用域写法：``with client.busy(): ...`` 期间为忙碌。可嵌套、可跨线程同时持有（引用计数），
        最后一个作用域退出（含异常退出）且 :meth:`set_busy` 开关为关时恢复空闲。"""
        return self._busy.scope()

    def is_busy(self) -> bool:
        """核心当前是否处于忙碌状态。"""
        return self._inner.is_busy()

    def set_busy_policy(self, policy: BusyPolicyLike) -> None:
        """修改忙碌期间写调用的处理方式（``"reject"`` / ``"queue"``），随即对排队中的调用生效。

        @error 未知策略抛 ``ValueError``。
        """
        self._inner.set_busy_policy(_busy_policy(policy))

    # -- 事件（spec/protocol.md 3.5） -------------------------------------------

    def declare_event(
        self, name: str, description: str, payload_schema: dict[str, Any] | str | None = None
    ) -> None:
        """声明本实例可发出的事件（同名替换）。已连接时随即同步给 Host，否则在下次握手成功后同步；不触发连接。

        ``payload_schema`` 为载荷的 JSON Schema（字典或 JSON 文本，描述用，Hub 不校验）。

        @error 名称不合法 → ``AppMcpError.InvalidName``；``payload_schema`` 不是合法 JSON → ``ValueError``；
        已停止 → ``AppMcpError.Stopped``。
        """
        info = ffi.EventInfo(name=name, description=description, payload_schema_json=_schema_json(payload_schema))
        self._inner.declare_event(info)

    def remove_event(self, name: str) -> bool:
        """撤销事件声明；未声明过（或已停止）返回 ``False``。"""
        return self._inner.remove_event(name)

    def emit_event(self, name: str, payload: Mapping[str, Any] | None = None) -> bool:
        """发出已声明的事件，``payload`` 为 JSON 对象（字典；``None`` = 无载荷）。

        已连接时发送并返回 ``True``；未连接（休眠、断线、重连中）丢弃并返回 ``False``：不缓存、不为此连接或唤醒 Host，
        也不推迟空闲休眠。需要可靠送达的状态变化请改用资源。

        @error 未声明、名称不合法 → ``AppMcpError.InvalidName``；载荷不能序列化、不是对象或序列化后超过 8 KiB →
        ``AppMcpError.InvalidJson``；已停止 → ``AppMcpError.Stopped``。
        """
        text = None
        if payload is not None:
            try:
                text = json.dumps(payload, default=to_jsonable, ensure_ascii=False)
            except (TypeError, ValueError) as e:
                raise ffi.AppMcpError.InvalidJson(f"事件载荷无法序列化为 JSON：{e}") from e
        return self._inner.emit_event(name, text)

    def set_visibility(self, visibility: str | ffi.Visibility, focused: bool = True) -> None:
        if isinstance(visibility, str):
            visibility = _VISIBILITIES[visibility.lower()]
        self._inner.set_visibility(visibility, focused)

    def _state_changed(self, state: ffi.StateInfo) -> None:
        with self._state_cond:
            self._state = state
            self._state_cond.notify_all()
        if self._on_state is not None:
            try:
                self._on_state(state)
            except Exception:
                logger.exception("on_state 回调抛出异常")

    # -- 执行 ----------------------------------------------------------------

    def _event_loop(self) -> asyncio.AbstractEventLoop:
        with self._loop_lock:
            if self._loop is None:
                loop = asyncio.new_event_loop()
                ready = threading.Event()

                def run() -> None:
                    asyncio.set_event_loop(loop)
                    loop.call_soon(ready.set)
                    loop.run_forever()

                t = threading.Thread(target=run, name="app-mcp-asyncio", daemon=True)
                t.start()
                ready.wait()
                self._loop = loop
                self._own_loop_thread = t
            return self._loop

    def _execute(
        self,
        reg: _Registration,
        prepare: Callable[[], dict[str, Any]],
        finish: Callable[[Any], None],
        fail: Callable[..., None],
        ctx: ToolContext | None,
    ) -> None:
        """在分发线程上调用：把函数投递到目标线程 / 事件循环执行。"""
        fn = reg.fn
        if reg.is_async:
            loop = self._event_loop()

            async def run_async() -> None:
                try:
                    kwargs = prepare()
                    result = await fn(**kwargs)
                except asyncio.CancelledError:
                    fail("CANCELLED", "调用已取消")
                    return
                except BaseException as e:  # noqa: BLE001 - 所有异常都要转为调用失败
                    fail(*_error_of(e))
                    return
                finish(result)

            future = asyncio.run_coroutine_threadsafe(run_async(), loop)
            if ctx is not None:
                ctx.on_cancel(lambda: future.cancel())
            return

        gate = _Gate()
        timer: threading.Timer | None = None
        if self._dispatch_timeout is not None:
            timeout = self._dispatch_timeout

            def on_timeout() -> None:
                if gate.abandon():
                    fail("APP_NOT_RESPONDING", f"{timeout:g} 秒内未能在目标线程上开始执行")

            timer = threading.Timer(timeout, on_timeout)
            timer.daemon = True
            timer.start()

        def job() -> None:
            if timer is not None:
                timer.cancel()
            if not gate.begin():
                return
            if ctx is not None and ctx.cancelled:
                return
            try:
                result = fn(**prepare())
            except BaseException as e:  # noqa: BLE001
                fail(*_error_of(e))
                return
            finish(result)

        try:
            self._dispatcher(job)
        except Exception as e:  # 调度器拒绝（例如线程池已关闭）
            if timer is not None:
                timer.cancel()
            if gate.abandon():
                fail("HANDLER_ERROR", f"无法调度 handler：{e}")
