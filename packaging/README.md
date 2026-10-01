# packaging：Host 预编译二进制的分发

`app-mcp-host` 的发布流水线与包管理器入口（`docs/plans/13-out-of-box.md` D1 / D2）。命令名 `appwire`，二进制仍为 `app-mcp-host`。

| 路径 | 内容 |
|---|---|
| `platforms.json` | 平台表（唯一定义）：runner、Rust target、二进制列表、npm `os`/`cpu`、wheel 平台标签 |
| `npm/` | npm 主包 `appwire-cli`：`appwire` / `appwire-cli` 命令，按 `process.platform-process.arch` 解析可选依赖 `appwire-cli-<平台>` 中的 `bin/app-mcp-host` 并执行 |
| `pypi/` | PyPI 包 `appwire-cli`：每个平台一个 `py3-none-<平台标签>` wheel，二进制在包内 `bin/`，POSIX 上 `execv`、Windows 上子进程并回传退出码 |
| `scripts/stage-npm.mjs` | 由构建产物生成平台包 + 主包（写入版本号与 `optionalDependencies`） |
| `scripts/build_wheels.py` | 由构建产物生成平台 wheel（先按纯 Python 包构建，再 `wheel tags` 改平台标签） |

为何放在仓库根 `packaging/` 而不是 `packages/`：`packages/*` 是 App 侧 SDK（pnpm workspace 成员，互相依赖）；这里是发布产物的包装，npm 与 PyPI 两种入口共用同一张平台表和同一份构建产物，放在一起；npm 启动器无依赖，不进 workspace，不需要改 `pnpm-lock.yaml`。

为何包名是 `appwire-cli`：npm 与 PyPI 上的 `appwire` 都已被他人占用（2026-10-02 查询，见 `TASKS.md` 第 13 项）。

## 发版

1. 根 `Cargo.toml` 的 `[workspace.package] version` 改为新版本并提交。
2. 推送 tag：`git tag v<版本> && git push origin v<版本>`（tag 与 workspace 版本不一致时流水线失败）。
3. `.github/workflows/release.yml`：6 个平台构建 → GitHub Release（`app-mcp-host-v<版本>-<平台>.tar.gz` / `.zip` + `SHA256SUMS`）→ npm → PyPI。
   发布步骤可重跑：npm 跳过已存在的 `包@版本`，PyPI `--skip-existing`。
4. 只想验证构建与打包、不发布：在 Actions 页手动运行 `release`（workflow_dispatch）。

## secrets（仓库 Settings → Secrets and variables → Actions）

| 名称 | 用途 | 缺省时 |
|---|---|---|
| `NPM_TOKEN` | npm 自动化令牌，需能发布 `appwire-cli` 与 `appwire-cli-{linux,win32,darwin}-{x64,arm64}` 共 7 个包 | 跳过 npm 发布（notice） |
| `PYPI_API_TOKEN` | PyPI API 令牌（首次发布需账号级令牌，之后可改为项目 `appwire-cli` 级） | 跳过 PyPI 发布（notice） |

GitHub Release 用内置 `GITHUB_TOKEN`（job 级 `contents: write`），无需配置。npm 发布带 `--provenance`（需公开仓库）。

## 本地测试

```bash
node_modules/.bin/vitest run --root packaging/npm
node_modules/.bin/tsc --noEmit -p packaging/npm/tsconfig.json
(cd packaging/pypi && python -m pytest -q)   # 需要 setuptools>=77、build、wheel
```

## 已知限制

- 未做代码签名 / 公证（Windows SmartScreen、macOS Gatekeeper），见计划 U2。
- 经 `npx` / `uvx` 临时运行时二进制在包管理器缓存中，`service install` 会把该路径写进登录自启项，缓存清理后失效：
  请用 `npx appwire-cli setup` / `uvx appwire-cli setup`（D3）——`setup` 检测到程序位于包管理器目录（`node_modules`、
  `site-packages`、`_npx`）时先复制到 `<home>/bin/`（默认 `~/.app-mcp/bin/`，Windows 连同 `app-mcp-hostw.exe`），
  再从那里注册自启，并写入已安装 Agent 的 MCP 配置；`appwire uninstall --purge` 撤销并删除该副本。见 `crates/host/README.md`。
- Linux 产物为 musl 静态链接（glibc / musl 发行版通用）；`linux-arm64` 依赖 GitHub 的 `ubuntu-24.04-arm` runner。
