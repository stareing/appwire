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
  (`USER_ACTION_REQUIRED`; `reason` and `uri` are optional and omitted when `None`).

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

## Embedding the Hub in your own agent

`app_mcp.hub.Hub` embeds the hub in a Python agent: export tools in MCP, OpenAI, Anthropic or
Gemini format, dispatch the model's tool calls, and handle approvals in your own UI. See
[`examples/hub_llm_loop.py`](examples/hub_llm_loop.py).

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
