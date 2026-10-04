# AppWire Python SDK

Part of [AppWire](https://github.com/stareing/appwire): expose the actions of a Python app (Qt, Tk,
asyncio or plain threads) as MCP (Model Context Protocol) tools for AI agents such as Claude, ChatGPT
and Gemini. The app connects to the local AppWire Host, which serves the tools to Claude Code,
Claude Desktop and any other MCP client.

## Usage

```python
from app_mcp import AppMcp

notes: list[str] = []
app = AppMcp("notes", "Notes", overview="A notebook: add, list and clear notes")

@app.tool("add", description="Add a note", risk="write")
def add(text: str) -> dict:
    notes.append(text)
    return {"count": len(notes)}

@app.tool("clear", description="Delete all notes", risk="destructive")
def clear() -> dict:
    notes.clear()
    return {"cleared": True}

app.start()
```

Handlers may be plain functions or `async def`; input schemas are derived from type hints (and
from Pydantic models with the `pydantic` extra). `app_mcp.dispatchers` runs handlers on the Qt or
Tk main thread, and `app_mcp.linux` provides D-Bus wake-up (`dbus` extra).

### Tool declarations and structured results (optional, `spec/protocol.md` §3.2)

```python
from app_mcp import ToolResult

@app.tool(
    "submit",
    description="Submit the order",
    annotations={"destructive_hint": True, "idempotent_hint": False, "open_world_hint": True},
    output_schema={"type": "object", "properties": {"order_id": {"type": "string"}}},
)
def submit() -> ToolResult:
    return ToolResult(
        None,
        status="pending",              # "done" (default) / "pending" / "partial" / "noop"
        state_resource="order_status", # resource to read for the eventual outcome
        summary="Submitted; waiting for the user to confirm in the app",
        annotations={"audience": ["user"]},
    )
```

- `annotations` are the standard MCP tool annotations (`ToolAnnotations`, or a dict with the keys `title`,
  `read_only_hint`, `destructive_hint`, `idempotent_hint`, `open_world_hint`). They are passed to the agent unchanged;
  when `risk` is also given, each declared field wins and missing ones are derived from `risk`. AppWire does not
  allow or block calls based on them.
- `output_schema` (dict or JSON text) is the result's JSON Schema; the Hub wraps a non-object root as `{result: …}`.
- `handle.update(...)` changes the declaration: an omitted keyword keeps the current value, an explicit `None` clears
  it (`title`, `activation`, `annotations`, `output_schema` are removed; `risk=None` restores the default,
  `input_schema=None` means no parameters). `description` cannot be cleared.
- Returning a plain value is a `done` result. Return `ToolResult` for a status, summary or content annotations
  (`ContentAnnotations`, or a dict with `audience` / `priority` / `last_modified`). With no return value (`None`,
  no `summary`, status `done`) the Hub shows the model the fixed text "已完成" ("done") instead of `null`.
- Raise `ToolCallError(kind, message, details=None)` to fail with a specific error kind. When only the user can
  unblock the call (expired login, missing OS permission, app must be in the foreground, in-app confirmation), raise
  `ToolCallError.user_action_required("Login expired, sign in again", UserActionReason.LOGIN, "notes://login")`
  (`USER_ACTION_REQUIRED`; `reason` and `uri` are optional and omitted when `None`). Resource readers can raise
  the same errors; the kind and details reach the Host unchanged.
- Resources take content annotations too: `add_resource(fn, "cart", annotations={"audience": ["user"], "priority": 0.5})`
  (or `@app.resource(..., annotations=...)`); the Hub puts them on the resource in MCP `resources/list`.
- Call de-duplication (`spec/protocol.md` §3.3): a retried `callId` replays the first result for 300 s / 64 entries by
  default. Tune or disable it with `AppMcp(..., call_dedup=CallDedup(ttl=60, max_entries=16))` /
  `call_dedup=CallDedup.OFF`; each hit is logged as a warning.
- Agent idempotency key (`spec/protocol.md` §3.3): a handler that takes `ctx: ToolContext` reads
  `ctx.idempotency_key` — the key the agent attached to the request, verbatim, or `None`. Repeats of the same key on
  the same tool are already replayed by the SDK like a retried `callId`; use the key yourself for business-level
  de-duplication (e.g. pass it to your backend).

### View tools and navigation (optional, `spec/protocol.md` §3.4)

- `add_tool(..., surface="view", page="cart")` declares a tool that only makes sense while its screen is visible;
  `page` tells the Hub where the tool lives. With Qt, `app_mcp.qt.bind_view_tool(widget, handle)` enables the tool
  while `widget` is shown, disables it when hidden and disposes it when the widget is destroyed (PySide6 or PyQt6).
- `app.set_navigation_handler(fn)` (or `@app.on_navigate`) lets the Hub open a page before calling a tool on it:
  `fn(page, params)` switches the UI and returns; raise `NavigationDenied("Finish the draft first")` to refuse, any
  other exception fails the navigation. Sync handlers run through `dispatcher` (use `qt_dispatcher()` /
  `tk_dispatcher(root)` for the UI thread). Set it before `start()`: the capability is announced in the handshake.
- In the background: when the app is hidden, a navigation that needs the foreground is answered at once with
  `USER_ACTION_REQUIRED` (reason `foreground`) unless `navigate_in_background` is on (`AppMcp(...,
  navigate_in_background=True)` / `set_navigate_in_background(True)`; desktop default is on). With it on, the handler
  decides and may raise `ToolCallError.user_action_required("…", UserActionReason.FOREGROUND, uri)` (e.g. after
  posting a notification). Make features that must work in the background `app` tools, or give a view tool
  `background_tool="<app tool>"` so the Hub calls that one instead. See `spec/protocol.md` §3.4 "后台与前台".

### Call scheduling (optional, `spec/protocol.md` §5.3)

- `add_tool(..., concurrency=1)` caps how many calls of this tool run at once (`0`, the default, means only
  `max_concurrent_calls` applies); `exclusive="doc"` puts tools in a mutual-exclusion group so at most one call of
  the group runs at a time. Calls that cannot start wait in arrival order; other tools are not blocked. Both stay
  inside the SDK and are not sent to the Host. `handle.update(concurrency=..., exclusive=None)` changes them later.
- `AppMcp(..., max_queued_calls=N)` bounds the wait queue (default 64, `0` = unlimited); a call arriving at a full
  queue fails with `RATE_LIMITED` (`data.scope == "queue"`).

### Standard intents (optional, `spec/intents.md`)

Declare that a tool implements a shared verb so agents can find it by action ("send a message") without knowing your
app first:

```python
@client.tool("mail.compose.send", "Send an email", implements=["message.send@1"])
def send(to: list[str], text: str, subject: str | None = None) -> dict:
    ...
```

- Each item is `"<verb>@<major version>"` (at most 4, no duplicates); a single string counts as one item.
  A malformed item raises `AppMcpError.InvalidName`. Unknown verbs, or an input schema missing the verb's required
  parameters (`message.send@1` needs `to` and `text`), only log a warning; the Hub then lists the tool as
  `known: false` or under `incompatible`.
- `handle.update(implements=[...])` replaces the list (`[]` clears it); other updates keep it.
- Agents list implementers with the built-in tool `apps.intents {intent?}` and then call the chosen tool by its full
  name as usual. The Hub never routes by verb.

### Result caching (optional, `spec/protocol.md` §3.6)

Let the Hub reuse a read-only result for a while instead of waking your app again:

```python
@client.tool("weather.today", "Today's forecast", annotations={"read_only_hint": True},
             cache={"ttl_ms": 600_000, "scope": "shared"})
def today(city: str) -> dict:
    ...
```

- `cache` is `{"ttl_ms": N, "scope": "private" | "shared"}`, a plain `ttl_ms` int, or a `CachePolicy`. `ttl_ms` must be
  1..86 400 000; out of range raises `AppMcpError.InvalidConfig`. `scope` defaults to `"private"` (per caller); use
  `"shared"` only for data that does not depend on who asks.
- Only tools whose effective annotations are read-only are cached (a write tool's `cache` is ignored with a warning).
  Resources take the same `cache=` argument.
- `handle.update(cache=...)` replaces it, `cache=None` clears it; other updates keep it. The SDK never caches anything
  itself.

### Deprecating a tool (optional, `spec/protocol.md` §3.7)

An incompatible schema change needs a new tool name; keep the old tool and mark it deprecated:

```python
@client.tool("search.v1", "Search (old)",
             deprecated={"message": "Use search.v2: supports paging", "replacement": "search.v2", "until": "2027-06-30"})
def search_v1(q: str) -> list:
    ...
```

- `deprecated` is `{"message", "replacement"?, "until"?}`, a plain string (the message), or a `Deprecation`. `message`
  is 1..500 characters, `replacement` a local tool name other than itself, `until` a `YYYY-MM-DD` date (a hint only);
  anything else raises `AppMcpError.InvalidConfig`.
- Deprecated tools are still listed and callable; agents see the notice. `handle.update(deprecated=...)` replaces it,
  `deprecated=None` clears it; other updates keep it.

### Undo (optional, `spec/protocol.md` §3.8)

Only the handler knows how to reverse what it just did, so it says so in the result; the hub records it and agents can
call `apps.undo`:

```python
@client.tool("todo.add", "Add a todo", undoable=True)
def add(text: str) -> ToolResult:
    item = todos.add(text)
    return ToolResult({"id": item.id},
                      undo={"tool": "todo.remove", "arguments": {"id": item.id}, "label": "Remove the todo just added"})
```

- `undo` is `{"tool", "arguments"?, "label"?}` or an `UndoAction`: `tool` is a local tool name in the same app (may be
  the tool itself, e.g. a toggle), `arguments` a JSON object (default `{}`), `label` 1..200 characters for the user.
- The undo call is an ordinary call of that tool; if the object has changed since, the handler returns an error or
  `noop`. Only `done` / `partial` results count. Invalid contents are dropped with a warning — the call still succeeds.
- `undoable=True` only tells agents "this can usually be undone"; `handle.update(undoable=False)` clears it.

### User is busy (optional, `spec/protocol.md` §5.3)

While the user is actively working in the app (a text field has focus, a drag is in progress — your call), declare it so
agent writes do not collide with their edits:

- `client.set_busy(True)` / `client.set_busy(False)`; `client.is_busy()` reads the current state.
- `with client.busy(): ...` marks a busy scope. Scopes nest and may be held from several threads (reference counted);
  the app is busy while the `set_busy` switch is on **or** any scope is open, so `set_busy(False)` does not end an open scope.
- While busy, write calls (tools whose effective annotations are not `readOnlyHint: true`) follow `busy_policy`:
  `AppMcp(..., busy_policy="reject")` (default) fails them with `RATE_LIMITED` (`data.scope == "busy"`, the same
  `callId` may be retried later); `"queue"` holds them and runs them in arrival order once the user is done.
  Read-only calls are never affected. `client.set_busy_policy("queue")` changes the policy at runtime (e.g. from a
  user setting). The busy state stays inside the SDK and is not sent to the Host.

### Events (optional, `spec/protocol.md` §3.5)

Tell agents that something happened (an order shipped, a download finished) instead of making them poll:

```python
client.declare_event("order.shipped", "An order was shipped", {"type": "object", "properties": {"orderId": {"type": "string"}}})
client.emit_event("order.shipped", {"orderId": "o1"})  # True = sent, False = not connected (dropped)
client.remove_event("order.shipped")
```

- Declarations (name rules as for tools; the schema only describes the payload) are synced to the Host after the
  handshake and whenever they change; declaring never triggers a connection.
- `emit_event(name, payload=None)` takes a JSON object (dict, at most 8 KiB serialized). While not connected the event
  is **dropped** and `False` is returned: nothing is queued, the Host is not woken and the idle timer is not extended.
  Use a resource for state that must be delivered reliably.
- Undeclared / invalid names raise `AppMcpError.InvalidName`; a payload that is not an object, cannot be serialized or
  is too large raises `AppMcpError.InvalidJson`. Nothing is sent in either case.
- Agents subscribe with the built-in tools `apps.events.subscribe` / `apps.events` (inbox) on the Hub side.

### Control fallback for Qt Widgets (optional, `spec/ui-fallback.md`)

For screens without declared tools, an opt-in fallback registers `ui.outline` / `ui.click` / `ui.fill` /
`ui.press` / `ui.scroll` / `ui.read` (behaviour, formats and errors are defined in `spec/ui-fallback.md`). It
walks the QWidget tree and acts through widget APIs only: no screenshots, no coordinates. Off by default; enable
it in debug builds or when the user opts in. Requires PySide6 or PyQt6 (`pip install app-mcp[qt]` installs
PySide6-Essentials).

```python
from app_mcp.uifallback.qt import QtUiFallback, QtUiFallbackOptions, declare_mcp_tools

fallback = QtUiFallback.enable(app)               # Qt GUI thread, after QApplication exists
declare_mcp_tools(clear_button, "cart.clear")     # shown as [已声明：cart.clear] in the outline
fallback.close()                                  # unregisters all ui.* tools
```

- Tools are `surface="view"` and enabled only while a top-level window is visible and not minimized (tracked via
  `QWindow` / application signals; no timers, threads or global event filters while idle). Call
  `fallback.refresh()` if your first window is shown without activation.
- The engine runs on the SDK's asyncio loop and hops to the GUI thread through a queued signal for each step, so it
  works whatever `dispatcher` you configured. An action that opens a modal `QDialog.exec()` is not waited on.
- Mapping: names from `accessibleName`, the Qt accessibility name or the buddy label (`QLabel.setBuddy` /
  `QFormLayout`); click → `QAbstractButton.click()` / `showMenu()`, `QAction.trigger()` (menus), tab selection,
  item-view selection; fill → `QLineEdit` (`selectAll` + `insert`, so validators apply and `textEdited` fires),
  `QTextEdit` / `QPlainTextEdit`, `QSpinBox` / `QDoubleSpinBox`, `QComboBox` (by item text, emits `activated`),
  `QAbstractSlider`; keys → `QKeyEvent` press/release via `QApplication.sendEvent`, Tab along the focus chain;
  scroll → scroll-bar page steps, `QScrollArea.ensureWidgetVisible`, `QAbstractItemView.scrollTo`.
- Popups (menus, combo lists) and modal dialogs occlude the windows below them. Password-mode `QLineEdit`s show
  `••••` and are never filled or sent keys. "Required" is not supported (Qt has no such property).
- Other toolkits: implement `UiElement` / `UiWindow` / `UiPlatform` from `app_mcp.uifallback` and reuse
  `UiInspector` + `UiFallbackTools`.

## Lifecycle and power

By default a client stays connected (`persistent`). Desktop apps on Linux that export the D-Bus
wake service can sleep when idle and be woken by the Host:

```python
from app_mcp import AppMcp
from app_mcp.linux import dbus_lifecycle, serve_dbus_wake

BUS = "org.example.Notes"
app = AppMcp("notes", "Notes", lifecycle=dbus_lifecycle(BUS))  # idle + D-Bus wake descriptor
service = serve_dbus_wake(app, BUS)
app.start()
```

`dbus_lifecycle(bus, **overrides)` is the desktop default (`mode="idle"`, 2 s merge window); any field
you pass wins. It is not applied automatically: an `idle` app without a wake path cannot be reached
after it sleeps, so plain `AppMcp(...)` stays `persistent`.

`LifecyclePolicy` fields (times in seconds; see `spec/lifecycle.md` §3, §11, §13):

| Field | Default | Meaning |
|---|---|---|
| `mode` | `"persistent"` | `"persistent"` / `"idle"` / `"on-demand"` |
| `host_absent_retries` | `3` | In `idle` / `on-demand`, give up and go dormant after this many "Host not running" failures; `0` = retry forever |
| `merge_window` | `2.0` | After a call or resource read, stay online at most this long (plus the Host lease) |
| `sleep_on_background` | `False` | Sleep as soon as the app is hidden and idle, without waiting for the lease |
| `legacy_timers` | `False` | Restore the pre-4e timer behaviour |

Other switches: `AppMcp(..., heartbeat="auto" | "always" | "off")` (default `"auto"`: no heartbeat
over local IPC or desktop loopback), and `@app.resource(..., realtime=True)` /
`add_resource(..., realtime=True)` for resources the model waits on. A subscription to a `realtime`
resource keeps the app online, and a change while the app is asleep reconnects it to push the update.
Ordinary resources (the default) do not block sleep; their changes are delivered on the next connection.

### Addressing by name (`spec/naming.md`)

Instead of connecting to the Host, an app can register a name with the system name service and let the
Hub dial it on demand; when the app is not running, the system starts it:

```python
app = AppMcp("notes", "Notes",
             lifecycle=LifecyclePolicy(mode="on-demand", residency="exit-when-idle"),
             register_name=True,            # Linux: D-Bus name dev.appmcp.App.notes; Windows: per-app named pipe
             name_instance=None,            # optional: also register appmcp://notes/<instance>
             on_idle_exit=quit_app)         # activated process: exit after the Hub closes the channel
```

Register the activation once with `app-mcp-host app install --app-id notes --exec /path/to/notes` and run the
Hub with `app-mcp-host serve --name-service`. Supported on Linux (D-Bus) and Windows (named pipes); on other
platforms the option is ignored with a log line. `name_instance` must match `[a-z][a-z0-9-]{0,31}` and not be
`"default"` (otherwise `AppMcpError.InvalidConfig`). See `examples/named_app.py`.

## Embedding the Hub in your own agent

`app_mcp.hub.Hub` embeds the hub in a Python agent: export tools in MCP, OpenAI, Anthropic or
Gemini format, dispatch the model's tool calls, and handle approvals in your own UI. See
[`examples/hub_llm_loop.py`](examples/hub_llm_loop.py).

Progress (`spec/hub-api.md` §3.12): `await hub.call_tool("files.export", on_progress=lambda u: print(u.progress, u.total,
u.message))` receives the progress the app reports (merged by the Hub) on the caller's event loop, all before the
result is returned.

Resource protection and result checks (`spec/hub-api.md` §3.11):

```python
from app_mcp.hub import Hub

hub = Hub(limits={"toolRatePerMinute": 60, "maxResultBytes": 1 << 20}, output_validation="reject")
```

| `limits` key (`LimitsConfig` field) | Default | Meaning |
|---|---|---|
| `toolRatePerMinute` / `toolRateBurst` (`tool_rate_per_minute` / `tool_rate_burst`) | 120 / 30 | Token bucket per (app, tool); over it → `RATE_LIMITED`; `0` per minute = unlimited |
| `appRatePerMinute` / `appRateBurst` (`app_rate_per_minute` / `app_rate_burst`) | 600 / 60 | Token bucket per app (all tools combined) |
| `maxArgumentsBytes` / `maxResultBytes` / `maxResourceBytes` (`max_*_bytes`) | 1 MiB / 4 MiB / 4 MiB | Over it → `PAYLOAD_TOO_LARGE` (never truncated); `0` = unlimited |

`output_validation` (`OutputValidation` or `"off"` / `"log"` / `"reject"`, default `"log"`) decides what happens
when a result does not match the tool's `outputSchema`: skip the check, log a warning, or end the call with
`HANDLER_ERROR`. Rejected calls still return a `CallResult` whose `error.kind` is `"RATE_LIMITED"` or
`"PAYLOAD_TOO_LARGE"`. `CallResult` also carries the app's `status` (`ResultStatus`), `state_resource`, `summary`
and `annotations`.

Pages, navigation and idempotency (`spec/hub-api.md` §3.14 / §3.15): `HubTool.surface` (`ToolSurface.APP` / `VIEW`)
and `HubTool.page` describe an app tool's UI dependency and page (`None` for built-in and upstream tools).
`Hub(navigate_timeout_ms=5000)` bounds automatic navigation (default 5 s). `await hub.call_tool("shop.order.submit",
{...}, idempotency_key="order-7")` passes the agent's idempotency key to the app unchanged (`ctx.idempotency_key`);
an invalid key ends the call with `INVALID_INPUT`. `priority=CallPriority.INTERACTIVE` (or `"interactive"` /
`"normal"` / `"background"`; `None` = normal) is passed to the app unchanged, whose call queue runs interactive calls
before background ones. When the Hub routes a view tool to its declared background tool,
`CallResult.routed_to` names the tool actually called. Built-in tools `apps.activate` / `apps.release` are always
listed, plus `apps.page` / `apps.navigate` when a page catalog exists. `apps.calls` lists the session's in-flight calls and
`apps.cancel {callId}` cancels one of them (the caller gets `CANCELLED`); `hub.status().calls` lists every in-flight
call as `CallStatus` (`call_id`, `name`, `caller`, `subject`, `state: CallState`, `elapsed_ms`, `instance_id`,
`progress` / `progress_total` / `progress_message`, `platform_state`).

App events (`spec/hub-api.md` §3.17): agents subscribe with `apps.events.subscribe {appId, event?, filter?}` and fetch
their inbox with `apps.events {max?}` (`apps.events.unsubscribe {subscriptionId}` to stop). Every validated event also
arrives as `HubEvent.APP_EVENT(event)` on `hub.on_event` / `hub.events()`, and `hub.set_event_handler(fn, dispatcher=None)`
calls `fn(AppEvent)` once per event whether or not anyone subscribed (`AppEvent`: `id`, `app_id`, `instance_id`, `name`,
`payload_json`, `at_ms`). Without a dispatcher `fn` runs on the Hub's app-connection thread and must return quickly;
`None` clears it. `hub.status().events` is an `EventsStatus` (`subscriptions: [EventSubscriptionStatus]` with
`delivered` / `dropped` / `pending`, plus `dropped_invalid`).
Limits: `Hub(event_limits={"maxSubscriptions": 8, "maxInboxEvents": 20})` (or an `EventLimitOverrides`; also
`inboxTtlMs`, `perSubscriptionPerMinute`; unset fields keep the defaults 32 / 100 / 24 h / 60, `0` per minute = unlimited).

Standard intents (`spec/intents.md` §4): `HubTool.implements` lists the intents an app tool declares. The owner's
defaults (verb, or `verb@major`, → full tool name) only mark and reorder `apps.intents` results (`default: true` first):
`Hub(intent_defaults={"message.send": "mail.compose.send"})` or `hub.set_intent_defaults({...})` at runtime (`{}`
clears). An invalid table raises `HubError.Tool` (`kind == "INVALID_INPUT"`) and the previous table stays; an invalid
`intent_defaults` at start-up starts with an empty table instead. `hub.intents()` (also `hub.status().intents`) returns
an `IntentsStatus` (`defaults`, `last_error`).

Read-only result cache (`spec/hub-api.md` §3.20): tools and resources whose app declares `cache` are answered from the
Hub's memory within the TTL without waking the app; `CallResult.cached_age_ms` is set on a hit. `call_tool(...,
cache_bypass=True)` skips the lookup and refreshes the entry. Limits: `Hub(result_cache={"maxEntries": 256})` (or a
`CacheLimitOverrides`; also `maxBytes`, `maxEntryBytes`; defaults 1024 / 8 MiB / 64 KiB, `maxEntries: 0` disables the
cache). `hub.status().cache` is a `CacheStatus` (`entries`, `bytes`, `hits`, `misses`, `evictions`).

Tool evolution (`spec/hub-api.md` §3.21): every app tool in `hub.tools()` carries `schema_hash` (changes when its
`inputSchema` / `outputSchema` change) and `deprecated` (the app's `Deprecation`, or `None`). Breaking or possibly
breaking changes to a tool's definition are kept in `hub.status().schema_changes` (last 32 `SchemaChangeRecord`s).

Undo (`spec/hub-api.md` §3.23): a result whose app gave an undo carries `undo` (`UndoOffer`: `label`,
`expires_in_ms`); `await hub.call_tool("apps.undo", {})` reverses the caller's latest one (or `{"callId": ...}`) once,
and its result has `undo_of` (the original call id). `HubTool.undoable` shows the declaration; `hub.status().undo` is an
`UndoStatus` (`ttl_ms`, `max_per_task`, `records`).

Policy hook points (`spec/hub-api.md` §3.13): `Hub(policy={"rules": [{"id": "no-pay", "action": "deny", "app": "shop",
"tool": "pay*"}]})` or `hub.set_policy(...)` at runtime. `hide` removes an app / tool from every list (calls get
`TOOL_NOT_FOUND`); `deny` makes calls fail with `POLICY_DENIED` (`details.ruleId`). No rules = unchanged behavior;
`hub.policy()` returns the active rules with hit counts.

## Build

The SDK wraps the Rust core through UniFFI. Generate the bindings first:

```bash
bash bindings/uniffi/scripts/generate.sh && bash bindings/hub-uniffi/scripts/generate.sh
```

See the [AppWire README](https://github.com/stareing/appwire#readme) for how the Host and MCP
clients fit together.

## License

MIT OR Apache-2.0
