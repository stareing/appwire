"""一致性用例 runner（Python）：按 ``conformance/cases/*.json`` 的 ``app`` 部分注册工具与资源，连接 fake_host
（``--case`` 模式，核对在 fake_host 内完成）。格式与约定见 conformance/README.md；参照 Rust runner
crates/native/tests/conformance.rs。

只跑部分用例：``APP_MCP_CONFORMANCE_CASES=handshake,errors pytest tests/test_conformance.py``。
fake_host 定位与 test_integration.py 相同（``APP_MCP_FAKE_HOST`` 或 cargo 构建）。
"""

from __future__ import annotations

import json
import os
import subprocess
import threading
import time
from pathlib import Path
from typing import Any

import pytest

from app_mcp import (
    AppMcp,
    CallDedup,
    LifecyclePolicy,
    NavigationDenied,
    ToolCallError,
    ToolContext,
    ToolHandle,
    ToolResult,
)

# @why 复用集成测试的 fake_host 定位 / 构建 fixture（不重复实现）
from test_integration import fake_host_bin  # noqa: F401

pytestmark = pytest.mark.integration

SDK = "python"
# 本 runner 支持的用例能力（``requires``），见 conformance/README.md 第 4 节。
FEATURES = frozenset(
    {
        "toolOptions",
        "mutate",
        "lifecycle",
        "wake",
        "richResult",
        "userAction",
        "progress",
        "resourceOptions",
        "readFailure",
        "surface",
        "navigation",
        "backgroundTool",
        "backgroundNavigation",
    }
)
ROOT = Path(__file__).resolve().parents[3]
CASES_DIR = ROOT / "conformance" / "cases"
REPORT_DIR = ROOT / "target" / "conformance"
# 用例协议字段（camelCase）→ Python SDK 工具注解字典键（snake_case）
_TOOL_ANNOTATION_KEYS = {
    "title": "title",
    "readOnlyHint": "read_only_hint",
    "destructiveHint": "destructive_hint",
    "idempotentHint": "idempotent_hint",
    "openWorldHint": "open_world_hint",
}
_CONTENT_ANNOTATION_KEYS = {"audience": "audience", "priority": "priority", "lastModified": "last_modified"}
VERDICT_OK = frozenset({"pass", "xfail", "xpass", "skip"})


def _case_paths() -> list[Path]:
    only = os.environ.get("APP_MCP_CONFORMANCE_CASES")
    ids = None if not only else {s.strip() for s in only.split(",") if s.strip()}
    return sorted(p for p in CASES_DIR.glob("*.json") if ids is None or p.stem in ids)


def _rename(value: dict[str, Any] | None, keys: dict[str, str]) -> dict[str, Any] | None:
    return None if value is None else {keys[k]: v for k, v in value.items()}


def _seconds(ms: Any, default: float) -> float:
    return default if ms is None else ms / 1000


class CaseApp:
    """一个用例的 App：客户端与已注册工具（mutate 用）。"""

    def __init__(self, addr: str, case: dict[str, Any]) -> None:
        self.client = AppMcp("conf", "Conformance", host_url=_host_url(addr), **_config_kwargs(case))
        self.tools: dict[str, ToolHandle] = {}
        self._lock = threading.Lock()

    def register_tool(self, decl: dict[str, Any]) -> None:
        handler = decl.get("handler") or {}
        runs = [0]
        runs_lock = threading.Lock()

        def fn(ctx: ToolContext) -> Any:
            with runs_lock:
                runs[0] += 1
                count = runs[0]
            return self._run_handler(handler, count, ctx)

        handle = self.client.add_tool(
            fn,
            decl["name"],
            decl["description"],
            input_schema=decl.get("inputSchema"),
            risk=decl.get("risk"),
            activation=decl.get("activation"),
            title=decl.get("title"),
            enabled=decl.get("enabled", True),
            annotations=_rename(decl.get("annotations"), _TOOL_ANNOTATION_KEYS),
            output_schema=decl.get("outputSchema"),
            surface=decl.get("surface"),
            page=decl.get("page"),
            background_tool=decl.get("backgroundTool"),
        )
        with self._lock:
            self.tools[decl["name"]] = handle

    def register_resource(self, decl: dict[str, Any]) -> None:
        spec = decl.get("read") or {}

        def reader() -> Any:
            if "return" in spec:
                return spec["return"]
            if spec.get("fail") is not None:
                f = spec["fail"]
                raise ToolCallError(f.get("kind", "HANDLER_ERROR"), f["message"], f.get("details"))
            if spec.get("userAction") is not None:
                u = spec["userAction"]
                raise ToolCallError.user_action_required(u["message"], u.get("reason"), u.get("uri"))
            raise RuntimeError(spec.get("throw", "读取失败"))

        self.client.add_resource(
            reader,
            decl["name"],
            decl["description"],
            mime_type=decl.get("mimeType"),
            realtime=decl.get("realtime", False),
            annotations=_rename(decl.get("annotations"), _CONTENT_ANNOTATION_KEYS),
        )

    def mutate(self, op: dict[str, Any]) -> None:
        """handler 的 ``mutate`` 操作（conformance/README.md 2.3）；Python 的 ``update`` 是补丁型，``None`` 即清除。"""
        kind, name = op["op"], op.get("name")
        if kind == "register":
            self.register_tool(op["tool"])
            return
        with self._lock:
            handle = self.tools[name]
        if kind == "update":
            keys = {
                "description": "description",
                "inputSchema": "input_schema",
                "risk": "risk",
                "activation": "activation",
                "title": "title",
                "outputSchema": "output_schema",
                "surface": "surface",
                "page": "page",
                "backgroundTool": "background_tool",
            }
            changes = {keys[k]: v for k, v in op["set"].items() if k in keys}
            if "annotations" in op["set"]:
                changes["annotations"] = _rename(op["set"]["annotations"], _TOOL_ANNOTATION_KEYS)
            handle.update(**changes)
        elif kind == "remove":
            handle.dispose()
            with self._lock:
                del self.tools[name]
        elif kind in ("enable", "disable"):
            handle.set_enabled(kind == "enable")
        else:
            raise ValueError(f"未知的 mutate 操作 {kind}")

    def navigate(self, pages: dict[str, Any], page: str, params: dict[str, Any] | None) -> None:
        """``app.navigation``（conformance/README.md 2.4）。"""
        spec = pages.get(page)
        if spec is None:
            raise LookupError(f"未知页面：{page}")
        for op in spec.get("mutate", []):
            self.mutate(op)
        if "throw" in spec:
            raise RuntimeError(spec["throw"])
        if "deny" in spec:
            raise NavigationDenied(spec["deny"])
        if "fail" in spec:
            raise RuntimeError(spec["fail"])
        # 与工具 handler 同一惯用法：抛 user_action_required（封装层映射为 USER_ACTION_REQUIRED，不是 NAVIGATION_FAILED）
        if spec.get("userAction") is not None:
            u = spec["userAction"]
            raise ToolCallError.user_action_required(u["message"], u.get("reason"), u.get("uri"))
        if spec.get("failParams") is True:
            raise RuntimeError("" if params is None else json.dumps(params, ensure_ascii=False))

    def _run_handler(self, spec: dict[str, Any], count: int, ctx: ToolContext) -> Any:
        """顺序：progress → delayMs → mutate → 结果（conformance/README.md 2.1）。"""
        for p in spec.get("progress", []):
            ctx.progress(p["progress"], p.get("total"), p.get("message"))
        if "delayMs" in spec:
            ctx.wait_cancelled(spec["delayMs"] / 1000)
        for op in spec.get("mutate", []):
            self.mutate(op)
        if "throw" in spec:
            raise RuntimeError(spec["throw"])
        if spec.get("userAction") is not None:
            u = spec["userAction"]
            raise ToolCallError.user_action_required(u["message"], u.get("reason"), u.get("uri"))
        if isinstance(spec.get("result"), dict):
            r = spec["result"]
            return ToolResult(
                r.get("data"),
                r.get("stateHints"),
                status=r.get("status", "done"),
                state_resource=r.get("stateResource"),
                summary=r.get("summary"),
                annotations=_rename(r.get("annotations"), _CONTENT_ANNOTATION_KEYS),
            )
        if "return" in spec:
            return spec["return"]
        if spec.get("echo") is True:
            return ctx.arguments
        if spec.get("counter") is True:
            return {"count": count}
        # returnNothing（以及未声明结果）：Python 的"无返回值"即函数返回 None。
        return None


def _host_url(addr: str) -> str:
    if ":" in addr and not addr.startswith(("unix:", "pipe:")):
        return f"ws://{addr}/app"
    return addr


def _config_kwargs(case: dict[str, Any]) -> dict[str, Any]:
    c = case.get("app", {}).get("config") or {}
    kwargs: dict[str, Any] = {}
    lc = c.get("lifecycle")
    if lc:
        d = LifecyclePolicy()
        kwargs["lifecycle"] = LifecyclePolicy(
            mode=lc.get("mode", d.mode),
            idle_timeout=_seconds(lc.get("idleTimeoutMs"), d.idle_timeout),
            grace=_seconds(lc.get("graceMs"), d.grace),
            merge_window=_seconds(lc.get("mergeWindowMs"), d.merge_window),
        )
    dedup = c.get("callDedup")
    if dedup:
        d = CallDedup()
        kwargs["call_dedup"] = CallDedup(
            ttl=_seconds(dedup.get("ttlMs"), d.ttl), max_entries=dedup.get("maxEntries", d.max_entries)
        )
    if "maxConcurrentCalls" in c:
        kwargs["max_concurrent_calls"] = c["maxConcurrentCalls"]
    if "navigateInBackground" in c:
        kwargs["navigate_in_background"] = c["navigateInBackground"]
    return kwargs


def run_case(fake_host: Path, path: Path) -> dict[str, Any]:
    """跑一个用例，返回 fake_host 给出的结论行。"""
    case = json.loads(path.read_text(encoding="utf-8"))
    missing = [f for f in case.get("requires", []) if f not in FEATURES]
    cmd = [str(fake_host), "--case", str(path), "--sdk", SDK, "--report-dir", str(REPORT_DIR)]
    if missing:
        cmd += ["--skip", f"runner 不支持：{', '.join(missing)}"]
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, text=True, encoding="utf-8")
    assert proc.stdout is not None
    app: CaseApp | None = None
    verdict: dict[str, Any] = {}
    try:
        for raw in proc.stdout:
            line = raw.strip()
            if line.startswith("LISTENING "):
                app = CaseApp(line.split(" ", 1)[1], case)
                for t in case["app"].get("tools", []):
                    app.register_tool(t)
                for r in case["app"].get("resources", []):
                    app.register_resource(r)
                pages = case["app"].get("navigation")
                if pages is not None:
                    nav_app = app
                    app.client.set_navigation_handler(lambda page, params: nav_app.navigate(pages, page, params))
                visibility = case["app"].get("visibility")
                if visibility is not None:
                    app.client.set_visibility(visibility, focused=False)
                app.client.start()
                continue
            try:
                v = json.loads(line)
            except ValueError:
                continue
            if v.get("type") == "wake" and app is not None:
                app.client.handle_wake(v.get("arg", ""))
            elif v.get("type") == "verdict":
                verdict = v
        proc.wait(timeout=60)
    finally:
        if app is not None:
            app.client.close()
        if proc.poll() is None:
            proc.kill()
    verdict["_exit"] = proc.returncode
    return verdict


@pytest.mark.parametrize("case_path", _case_paths(), ids=lambda p: p.stem)
def test_conformance_case(fake_host_bin: Path, case_path: Path) -> None:
    started = time.monotonic()
    v = run_case(fake_host_bin, case_path)
    status = v.get("status", "error")
    print(f"[{SDK}] {case_path.stem:<24} {status} ({time.monotonic() - started:.1f}s)")
    assert status in VERDICT_OK and v["_exit"] == 0, (
        f"{case_path.stem}: {status}（退出码 {v['_exit']}）\n"
        f"{json.dumps(v.get('failures'), ensure_ascii=False, indent=2)}\n"
        f"详情见 {REPORT_DIR / SDK / (case_path.stem + '.json')}"
    )
