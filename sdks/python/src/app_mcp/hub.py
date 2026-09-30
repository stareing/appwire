"""Hub SDK（Agent 端）：把"连接本机所有 App"的能力嵌进自己的助手 / Agent。

列工具、调用、导出 / 分派 LLM 工具格式（OpenAI、Anthropic、Gemini、MCP）、事件、审批与配对回调。
需要先运行 ``bindings/hub-uniffi/scripts/generate.sh`` 生成 ``app_mcp_hub`` 包里的绑定与原生库。

>>> from app_mcp.hub import Hub
>>> hub = Hub(approval_min_risk="destructive")
>>> hub.set_approval_handler(lambda req: ask_user(req))        # 同步或 async 均可
>>> tools = hub.export_tools("anthropic")                      # 放进 LLM 请求
>>> result = await hub.dispatch("anthropic", tool_use_block)   # 回填给 LLM
>>> hub.close()

线程模型：

- async 方法（``call_tool``、``dispatch``、``read_resource``、``serve_http``）在调用方的事件循环上等待，
  不阻塞；对应的 ``*_sync`` 版本供非 asyncio 代码使用（不要在事件循环线程上调用）。
- 事件回调在 Hub 的分发线程上同步执行（可传 ``dispatcher`` 切到 UI 线程，见 ``app_mcp.dispatchers``）；
  ``events()`` 提供 async 迭代器。
- 审批 / 配对 / 唤醒回调：同步函数在线程池（或 ``dispatcher``）中执行，可以阻塞等待用户；
  async 函数在设置回调时所在的事件循环（或 ``loop`` 参数）上执行。

休眠与唤醒（spec/hub-api.md 3.5）：App 休眠后其工具仍列出（``Availability.DORMANT``），
``AppInfo.dormant_instances`` 列出休眠实例，并收到 ``HubEvent.APP_DORMANT``；调用这些工具时 Hub 生成一次性令牌、
发 ``HubEvent.APP_WAKING`` 并调用唤醒实现（默认按平台执行系统命令；可用 :meth:`Hub.set_waker` 替换）。
"""

from __future__ import annotations

import asyncio
import concurrent.futures
import inspect
import json
import logging
import threading
from collections.abc import Awaitable, Callable
from dataclasses import dataclass, field
from typing import Any, Union

try:
    from app_mcp_hub import app_mcp_hub_uniffi as ffi
except ImportError as e:  # pragma: no cover - 取决于是否已生成
    raise ImportError(
        "找不到 app_mcp_hub 绑定，请先运行 bash bindings/hub-uniffi/scripts/generate.sh"
    ) from e

_log = logging.getLogger("app_mcp.hub")

# 直接复用生成的数据类型。
HubConfig = ffi.HubConfig
UpstreamSpec = ffi.UpstreamSpec
ToolFilter = ffi.ToolFilter
ToolFormat = ffi.ToolFormat
Risk = ffi.Risk
Activation = ffi.Activation
Availability = ffi.Availability
Visibility = ffi.Visibility
AppKind = ffi.AppKind
AppInfo = ffi.AppInfo
InstanceInfo = ffi.InstanceInfo
AppOverviewInfo = ffi.AppOverviewInfo
HubTool = ffi.HubTool
HubResource = ffi.HubResource
ResourceContent = ffi.ResourceContent
ToolErrorInfo = ffi.ToolErrorInfo
ApprovalRequest = ffi.ApprovalRequest
PairingRequest = ffi.PairingRequest
#: 事件（``HubEvent.APP_CONNECTED`` 等变体；未单独映射的新事件为 ``HubEvent.OTHER(kind, json)``）。
HubEvent = ffi.HubEvent
#: Hub 操作错误（``HubError.Tool``、``InvalidJson``、``InvalidConfig``、``Io``、``Shutdown``）。
HubError = ffi.HubError
WakeKind = ffi.WakeKind
WakeDescriptor = ffi.WakeDescriptor
#: 交给唤醒回调的请求（``app_id``、``instance_id``、``descriptor``、``token``、``activation_arg``）。
WakeRequest = ffi.WakeRequest

FormatLike = Union[ToolFormat, str]
RiskLike = Union[Risk, str]
Dispatcher = Callable[[Callable[[], None]], None]

__all__ = [
    "Activation",
    "AppInfo",
    "AppKind",
    "AppOverviewInfo",
    "ApprovalRequest",
    "Availability",
    "CallResult",
    "EventStream",
    "Hub",
    "HubConfig",
    "HubError",
    "HubEvent",
    "HubResource",
    "HubTool",
    "InstanceInfo",
    "PairingRequest",
    "ResourceContent",
    "Risk",
    "ToolError",
    "ToolErrorInfo",
    "ToolFilter",
    "ToolFormat",
    "UpstreamSpec",
    "Visibility",
    "WakeDescriptor",
    "WakeFailed",
    "WakeKind",
    "WakeRequest",
    "init_logging",
    "parse_format",
]


# ---------------------------------------------------------------------------
# 事件循环：Rust 线程上触发的 async 回调需要一个事件循环
# ---------------------------------------------------------------------------

_cb_loop: asyncio.AbstractEventLoop | None = None
_cb_thread: threading.Thread | None = None
_cb_lock = threading.Lock()


def _callback_loop() -> asyncio.AbstractEventLoop:
    """后台守护线程上的事件循环：执行 Rust 线程发起的 async 回调，以及 ``*_sync`` 方法。"""
    global _cb_loop, _cb_thread
    with _cb_lock:
        if _cb_loop is None:
            loop = asyncio.new_event_loop()
            ready = threading.Event()

            def run() -> None:
                asyncio.set_event_loop(loop)
                loop.call_soon(ready.set)
                loop.run_forever()

            _cb_thread = threading.Thread(target=run, name="app-mcp-hub-loop", daemon=True)
            _cb_thread.start()
            ready.wait()
            _cb_loop = loop
        return _cb_loop


def _uniffi_event_loop() -> asyncio.AbstractEventLoop:
    # 生成代码默认只认 uniffi_set_event_loop 设置的全局循环或当前运行中的循环；
    # Rust 线程（无运行中的循环）触发的 async 回调改用后台循环，调用方的 await 仍在调用方循环上。
    if ffi._UNIFFI_GLOBAL_EVENT_LOOP is not None:
        return ffi._UNIFFI_GLOBAL_EVENT_LOOP
    try:
        return asyncio.get_running_loop()
    except RuntimeError:
        return _callback_loop()


ffi._uniffi_get_event_loop = _uniffi_event_loop


def _on_callback_thread() -> bool:
    return _cb_thread is not None and threading.current_thread() is _cb_thread


# ---------------------------------------------------------------------------
# 工具函数
# ---------------------------------------------------------------------------


def parse_format(value: FormatLike) -> ToolFormat:
    """``ToolFormat`` 或格式名（``mcp``、``openai-chat``/``openai``、``openai-responses``、``anthropic``、``gemini``）。"""
    if isinstance(value, ToolFormat):
        return value
    return ffi.parse_tool_format(value)


def _risk(value: RiskLike | None) -> Risk | None:
    if value is None or isinstance(value, Risk):
        return value
    return Risk[value.strip().upper().replace("-", "_")]


def init_logging(filter: str | None = None) -> bool:
    """Hub 日志输出到 stderr（``RUST_LOG`` 语法）。只有第一次调用生效。"""
    return ffi.init_logging(filter)


def _loads(text: str | None) -> Any:
    return None if text is None else json.loads(text)


def _millis(seconds: float | None) -> int | None:
    return None if seconds is None else max(0, int(seconds * 1000))


def _filter(
    apps: list[str] | None = None,
    max_risk: RiskLike | None = None,
    only_available: bool = False,
    include_builtin: bool = True,
) -> ToolFilter:
    return ToolFilter(
        apps=apps, max_risk=_risk(max_risk), only_available=only_available, include_builtin=include_builtin
    )


class ToolError(Exception):
    """工具调用以错误结束（``CallResult.unwrap()``）。``kind`` 为协议错误类别，如 ``USER_REJECTED``。"""

    def __init__(self, kind: str, message: str, details: Any = None) -> None:
        super().__init__(f"{kind}: {message}")
        self.kind = kind
        self.message = message
        self.details = details


@dataclass
class CallResult:
    """一次工具调用的结果。"""

    call_id: str
    #: 成功时的结果数据（已解析的 JSON）。
    data: Any = None
    #: 失败时的错误（``error.kind`` 如 ``USER_REJECTED``、``TIMEOUT``）。
    error: ToolErrorInfo | None = None
    state_hints: list[str] = field(default_factory=list)
    #: 实际执行的实例。
    instance_id: str | None = None
    #: 本会话首次接触该 App 时附带的总览。
    overview: AppOverviewInfo | None = None

    @property
    def ok(self) -> bool:
        return self.error is None

    def unwrap(self) -> Any:
        """成功时返回数据，失败时抛出 :class:`ToolError`。"""
        if self.error is not None:
            raise ToolError(self.error.kind, self.error.message, _loads(self.error.details_json))
        return self.data


class WakeFailed(Exception):
    """在唤醒回调中抛出，以指定的协议错误类别结束调用（其他异常按 ``LAUNCH_FAILED``）。"""

    def __init__(self, kind: str, message: str) -> None:
        super().__init__(f"{kind}: {message}")
        self.kind = kind
        self.message = message


async def _invoke(
    fn: Callable[[Any], Any],
    arg: Any,
    loop: asyncio.AbstractEventLoop | None,
    dispatcher: Dispatcher | None,
) -> Any:
    """在合适的上下文执行用户回调（同步 / async），返回结果；异常原样抛出。"""
    if inspect.iscoroutinefunction(fn):
        coro = fn(arg)
        if loop is not None and loop is not asyncio.get_running_loop():
            return await asyncio.wrap_future(asyncio.run_coroutine_threadsafe(coro, loop))
        return await coro
    if dispatcher is not None:
        fut: concurrent.futures.Future[Any] = concurrent.futures.Future()

        def run() -> None:
            try:
                fut.set_result(fn(arg))
            except BaseException as e:  # noqa: BLE001 - 转交给等待方
                fut.set_exception(e)

        dispatcher(run)
        result = await asyncio.wrap_future(fut)
    else:
        result = await asyncio.get_running_loop().run_in_executor(None, fn, arg)
    if inspect.isawaitable(result):
        result = await result
    return result


async def _invoke_bool(
    fn: Callable[[Any], bool | Awaitable[bool]],
    arg: Any,
    loop: asyncio.AbstractEventLoop | None,
    dispatcher: Dispatcher | None,
) -> bool:
    """在合适的上下文执行用户回调（同步 / async），异常视为 False。"""
    try:
        return bool(await _invoke(fn, arg, loop, dispatcher))
    except Exception:
        _log.exception("回调抛出异常，视为拒绝")
        return False


class _ApprovalAdapter(ffi.ApprovalHandler):
    def __init__(self, fn: Any, loop: Any, dispatcher: Any) -> None:
        self._fn, self._loop, self._dispatcher = fn, loop, dispatcher

    async def approve(self, request: ApprovalRequest) -> bool:
        return await _invoke_bool(self._fn, request, self._loop, self._dispatcher)


class _PairingAdapter(ffi.PairingHandler):
    def __init__(self, fn: Any, loop: Any, dispatcher: Any) -> None:
        self._fn, self._loop, self._dispatcher = fn, loop, dispatcher

    async def pair(self, request: PairingRequest) -> bool:
        return await _invoke_bool(self._fn, request, self._loop, self._dispatcher)


class _WakerAdapter(ffi.HubWaker):
    def __init__(self, fn: Any, loop: Any, dispatcher: Any) -> None:
        self._fn, self._loop, self._dispatcher = fn, loop, dispatcher

    async def wake(self, request: WakeRequest) -> None:
        try:
            await _invoke(self._fn, request, self._loop, self._dispatcher)
        except WakeFailed as e:
            raise ffi.WakeError.Failed(kind=e.kind, reason=e.message) from None
        except Exception as e:
            _log.exception("唤醒回调抛出异常")
            raise ffi.WakeError.Failed(kind="LAUNCH_FAILED", reason=str(e) or type(e).__name__) from None


class _Listener(ffi.HubEventListener):
    def __init__(self, hub: Hub) -> None:
        self._hub = hub

    def on_event(self, event: HubEvent) -> None:  # 分发线程
        self._hub._emit(event)

    def on_lagged(self, skipped: int) -> None:  # 分发线程
        self._hub._emit_lagged(skipped)


class EventStream:
    """事件的 async 迭代器（创建时即开始接收）。``async for e in hub.events(): ...``"""

    def __init__(self, hub: Hub, loop: asyncio.AbstractEventLoop) -> None:
        self._loop = loop
        self._queue: asyncio.Queue[HubEvent | None] = asyncio.Queue()
        self._unsubscribe = hub.on_event(self._push)

    def _push(self, event: HubEvent) -> None:
        try:
            self._loop.call_soon_threadsafe(self._queue.put_nowait, event)
        except RuntimeError:  # 循环已关闭
            pass

    def __aiter__(self) -> EventStream:
        return self

    async def __anext__(self) -> HubEvent:
        item = await self._queue.get()
        if item is None:
            raise StopAsyncIteration
        return item

    async def next(self, timeout: float | None = None) -> HubEvent:
        """取下一个事件；超时抛出 ``TimeoutError``。"""
        return await asyncio.wait_for(self.__anext__(), timeout)

    async def wait_for(self, pred: Callable[[HubEvent], bool], timeout: float | None = None) -> HubEvent:
        """等待第一个满足 ``pred`` 的事件（其间的事件被丢弃）。"""

        async def loop() -> HubEvent:
            async for e in self:
                if pred(e):
                    return e
            raise StopAsyncIteration

        return await asyncio.wait_for(loop(), timeout)

    def close(self) -> None:
        self._unsubscribe()
        self._loop.call_soon_threadsafe(self._queue.put_nowait, None)

    def __enter__(self) -> EventStream:
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()


# ---------------------------------------------------------------------------
# Hub
# ---------------------------------------------------------------------------


class Hub:
    """嵌入式 Hub。

    参数与 :class:`HubConfig` 字段一致（``approval_min_risk`` 可用字符串，如 ``"destructive"``）；
    也可直接传 ``config=HubConfig(...)``。
    """

    def __init__(self, config: HubConfig | None = None, **kwargs: Any) -> None:
        if config is None:
            if "approval_min_risk" in kwargs:
                kwargs["approval_min_risk"] = _risk(kwargs["approval_min_risk"])
            config = HubConfig(**kwargs)
        elif kwargs:
            raise TypeError("config 与关键字参数不能同时使用")
        self._inner = ffi.AppMcpHub.start(config)
        self._subs: list[Callable[[HubEvent], None]] = []
        self._lagged_subs: list[Callable[[int], None]] = []
        self._subs_lock = threading.Lock()
        self._closed = False
        self._inner.set_event_listener(_Listener(self))

    # -- 生命周期 ---------------------------------------------------------------

    def close(self) -> None:
        """停止 Hub（断开所有 App、结束后台任务）。可重复调用。"""
        if self._closed:
            return
        self._closed = True
        try:
            self._inner.set_event_listener(None)
        finally:
            self._inner.shutdown()

    def __enter__(self) -> Hub:
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()

    async def __aenter__(self) -> Hub:
        return self

    async def __aexit__(self, *exc: object) -> None:
        self.close()

    @property
    def ws_addr(self) -> str | None:
        """App 连接服务的实际地址（端口 0 时为随机端口）；未开启时为 ``None``。"""
        return self._inner.ws_addr()

    # -- 查询 -------------------------------------------------------------------

    def apps(self) -> list[AppInfo]:
        return self._inner.apps()

    def tools(
        self,
        *,
        apps: list[str] | None = None,
        max_risk: RiskLike | None = None,
        only_available: bool = False,
        include_builtin: bool = True,
    ) -> list[HubTool]:
        return self._inner.tools(_filter(apps, max_risk, only_available, include_builtin))

    def resources(self) -> list[HubResource]:
        return self._inner.resources()

    def overview(self, app_id: str) -> AppOverviewInfo | None:
        return self._inner.overview(app_id)

    def select_instance(self, app_id: str, instance_id: str | None) -> None:
        """设置全局默认实例（``None`` 恢复按规则路由）。"""
        self._inner.select_instance(app_id, instance_id)

    # -- 调用 -------------------------------------------------------------------

    async def call_tool(
        self,
        name: str,
        arguments: dict[str, Any] | None = None,
        *,
        instance_id: str | None = None,
        timeout: float | None = None,
        session: str | None = None,
        call_id: str | None = None,
    ) -> CallResult:
        """调用工具（全名 ``<appId>.<tool>``，``timeout`` 单位秒）。

        工具层面的失败（用户拒绝、超时、App 报错……）放在 ``CallResult.error``；
        名称无法解析时抛出 :data:`HubError`。任务被取消时自动取消调用。
        """
        req = ffi.CallRequest(
            name=name,
            arguments_json=None if arguments is None else json.dumps(arguments, ensure_ascii=False),
            instance_id=instance_id,
            timeout_ms=_millis(timeout),
            call_id=call_id,
            session=session,
        )
        out = await self._inner.call_tool(req)
        return CallResult(
            call_id=out.call_id,
            data=_loads(out.data_json),
            error=out.error,
            state_hints=list(out.state_hints),
            instance_id=out.instance_id,
            overview=out.overview,
        )

    def call_tool_sync(self, name: str, arguments: dict[str, Any] | None = None, **kwargs: Any) -> CallResult:
        """:meth:`call_tool` 的阻塞版本（不要在事件循环线程上调用）。"""
        return self._run_sync(self.call_tool(name, arguments, **kwargs))

    def cancel_call(self, call_id: str) -> None:
        self._inner.cancel_call(call_id)

    async def read_resource(self, uri: str) -> ResourceContent:
        return await self._inner.read_resource(uri)

    def subscribe(self, uri: str) -> None:
        self._inner.subscribe(uri)

    def unsubscribe(self, uri: str) -> None:
        self._inner.unsubscribe(uri)

    # -- LLM 工具格式 -------------------------------------------------------------

    def export_tools_json(
        self,
        format: FormatLike,
        *,
        apps: list[str] | None = None,
        max_risk: RiskLike | None = None,
        only_available: bool = False,
        include_builtin: bool = True,
    ) -> str:
        """导出工具定义（JSON 文本）。"""
        return self._inner.export_tools(
            parse_format(format), _filter(apps, max_risk, only_available, include_builtin)
        )

    def export_tools(self, format: FormatLike, **filter: Any) -> Any:
        """导出工具定义（已解析），直接放进 LLM 请求的 ``tools``。

        Gemini 格式为 ``{"functionDeclarations": [...]}``，其余为列表。
        """
        return json.loads(self.export_tools_json(format, **filter))

    async def dispatch(self, format: FormatLike, tool_call: Any, session: str | None = None) -> Any:
        """执行模型发出的一个工具调用（dict，或该格式的 JSON 文本），返回应回填给模型的 dict。

        例如 Anthropic：``{"type": "tool_use", "id", "name", "input"}`` →
        ``{"type": "tool_result", "tool_use_id", "content", "is_error"?}``。
        """
        text = tool_call if isinstance(tool_call, str) else json.dumps(tool_call, ensure_ascii=False)
        return json.loads(await self._inner.dispatch(parse_format(format), text, session))

    def dispatch_sync(self, format: FormatLike, tool_call: Any, session: str | None = None) -> Any:
        """:meth:`dispatch` 的阻塞版本（不要在事件循环线程上调用）。"""
        return self._run_sync(self.dispatch(format, tool_call, session))

    def reset_session(self, session: str | None = None) -> None:
        """清除会话状态（首次接触总览、会话内 ``apps.select``）。"""
        self._inner.reset_session(session)

    async def serve_http(self, addr: str, allow_remote: bool = False) -> str:
        """同时以 MCP Streamable HTTP 对外提供，返回实际地址。"""
        return await self._inner.serve_http(addr, allow_remote)

    # -- 回调 -------------------------------------------------------------------

    def set_approval_handler(
        self,
        handler: Callable[[ApprovalRequest], bool | Awaitable[bool]],
        *,
        loop: asyncio.AbstractEventLoop | None = None,
        dispatcher: Dispatcher | None = None,
    ) -> None:
        """调用确认（风险不低于 ``approval_min_risk`` 时询问）。返回 False 或抛出异常 → ``USER_REJECTED``。

        同步 handler 默认在线程池中执行（可阻塞等待用户），传 ``dispatcher`` 则切到 UI 线程；
        async handler 在 ``loop``（缺省为调用本方法时正在运行的循环）上执行。
        """
        self._inner.set_approval_handler(_ApprovalAdapter(handler, loop or _running_loop(), dispatcher))

    def set_pairing_handler(
        self,
        handler: Callable[[PairingRequest], bool | Awaitable[bool]],
        *,
        loop: asyncio.AbstractEventLoop | None = None,
        dispatcher: Dispatcher | None = None,
    ) -> None:
        """App 配对确认。返回 False 或抛出异常 → 拒绝。执行上下文同 :meth:`set_approval_handler`。"""
        self._inner.set_pairing_handler(_PairingAdapter(handler, loop or _running_loop(), dispatcher))

    def set_waker(
        self,
        handler: Callable[[WakeRequest], None | Awaitable[None]] | None,
        *,
        loop: asyncio.AbstractEventLoop | None = None,
        dispatcher: Dispatcher | None = None,
    ) -> None:
        """自定义唤醒（如 Android 厂商发送显式广播），替换默认的系统唤醒实现；``None`` 恢复默认。

        正常返回表示已发出激活，Hub 随后等待 App 回连（``wake_timeout_ms``）；抛出 :class:`WakeFailed`
        以指定类别（如 ``APP_NOT_INSTALLED``）结束调用，其他异常按 ``LAUNCH_FAILED``。执行上下文同
        :meth:`set_approval_handler`。
        """
        self._inner.set_waker(
            None if handler is None else _WakerAdapter(handler, loop or _running_loop(), dispatcher)
        )

    def on_event(
        self, callback: Callable[[HubEvent], None], *, dispatcher: Dispatcher | None = None
    ) -> Callable[[], None]:
        """订阅事件，返回取消订阅函数。回调默认在 Hub 分发线程上执行，须尽快返回。"""
        cb = callback if dispatcher is None else (lambda e: dispatcher(lambda: callback(e)))
        with self._subs_lock:
            self._subs.append(cb)

        def unsubscribe() -> None:
            with self._subs_lock:
                if cb in self._subs:
                    self._subs.remove(cb)

        return unsubscribe

    def on_lagged(self, callback: Callable[[int], None]) -> Callable[[], None]:
        """事件处理过慢被跳过时回调（参数为跳过的数量）；收到后应重新拉取 ``apps()`` / ``tools()``。"""
        with self._subs_lock:
            self._lagged_subs.append(callback)

        def unsubscribe() -> None:
            with self._subs_lock:
                if callback in self._lagged_subs:
                    self._lagged_subs.remove(callback)

        return unsubscribe

    def events(self, loop: asyncio.AbstractEventLoop | None = None) -> EventStream:
        """事件的 async 迭代器（需要在事件循环中调用，或传 ``loop``）。"""
        return EventStream(self, loop or asyncio.get_running_loop())

    # -- 内部 -------------------------------------------------------------------

    def _emit(self, event: HubEvent) -> None:
        with self._subs_lock:
            subs = list(self._subs)
        for cb in subs:
            try:
                cb(event)
            except Exception:
                _log.exception("事件回调抛出异常")

    def _emit_lagged(self, skipped: int) -> None:
        with self._subs_lock:
            subs = list(self._lagged_subs)
        for cb in subs:
            try:
                cb(skipped)
            except Exception:
                _log.exception("lagged 回调抛出异常")

    @staticmethod
    def _run_sync(coro: Awaitable[Any]) -> Any:
        if _on_callback_thread():
            raise RuntimeError("不能在 Hub 回调循环上调用 *_sync 方法，请改用 async 版本")
        try:
            asyncio.get_running_loop()
        except RuntimeError:
            pass
        else:
            if inspect.iscoroutine(coro):
                coro.close()
            raise RuntimeError("事件循环线程上请直接 await async 版本")
        return asyncio.run_coroutine_threadsafe(coro, _callback_loop()).result()  # type: ignore[arg-type]


def _running_loop() -> asyncio.AbstractEventLoop | None:
    try:
        return asyncio.get_running_loop()
    except RuntimeError:
        return None
