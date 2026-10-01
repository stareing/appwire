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

## Embedding the Hub in your own agent

`app_mcp.hub.Hub` embeds the hub in a Python agent: export tools in MCP, OpenAI, Anthropic or
Gemini format, dispatch the model's tool calls, and handle approvals in your own UI. See
[`examples/hub_llm_loop.py`](examples/hub_llm_loop.py).

## Build

The SDK wraps the Rust core through UniFFI. Generate the bindings first:

```bash
bash bindings/uniffi/scripts/generate.sh && bash bindings/hub-uniffi/scripts/generate.sh
```

See the [AppWire README](https://github.com/stareing/appwire#readme) for how the Host and MCP
clients fit together.

## License

MIT OR Apache-2.0
