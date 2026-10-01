# appwire-cli

The command line of [AppWire](https://github.com/stareing/appwire): runs the prebuilt **AppWire Host** (`app-mcp-host`),
the local MCP server that exposes the tools of every app on this device to Claude, ChatGPT, Gemini and other MCP clients.

```bash
uv tool install appwire-cli     # or: pipx install appwire-cli
appwire service install         # start the Host at login (current user)
appwire doctor                  # check Host, ports, IPC and connected apps
```

One-off commands also work without installing: `uvx appwire-cli doctor`. Do not run `service install` through `uvx`:
the login service would point at a binary in uv's cache, which uv may delete.
Every argument is passed to `app-mcp-host` unchanged; run `appwire --help` for all subcommands.

Each wheel contains the binary for one platform; nothing is downloaded at run time.
Supported: Linux x64 / arm64 (static, glibc or musl), Windows x64 / arm64, macOS x64 / arm64.

License: MIT OR Apache-2.0.
