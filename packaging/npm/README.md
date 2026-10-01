# appwire-cli

The command line of [AppWire](https://github.com/stareing/appwire): runs the prebuilt **AppWire Host** (`app-mcp-host`),
the local MCP server that exposes the tools of every app on this device to Claude, ChatGPT, Gemini and other MCP clients.

```bash
npm install -g appwire-cli
appwire service install   # start the Host at login (current user)
appwire doctor            # check Host, ports, IPC and connected apps
```

One-off commands also work without installing: `npx appwire-cli doctor`. Do not run `service install` through `npx`:
the login service would point at a binary in the npx cache, which npm may delete.
Every argument is passed to `app-mcp-host` unchanged; run `appwire --help` for all subcommands.

The binary comes from a per-platform optional dependency (`appwire-cli-<platform>-<arch>`), so nothing is downloaded at run time.
Supported: Linux x64 / arm64 (static, glibc or musl), Windows x64 / arm64, macOS x64 / arm64.
Installing with `--omit=optional` / `--no-optional` skips the binary; reinstall without that flag.

License: MIT OR Apache-2.0.
