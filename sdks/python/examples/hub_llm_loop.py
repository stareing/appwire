"""示例：自有 LLM 循环 + 嵌入式 Hub（不走 MCP）。

厂商助手把 Hub 嵌进自己的进程：``export_tools`` 得到 LLM 工具定义，模型发出工具调用后
``dispatch`` 执行并得到可直接回填的结果；高风险调用由自己的 UI 审批。

为了能独立运行，本示例在同一进程里再起一个演示 App（app_mcp.AppMcp，经 WebSocket 连上 Hub）。
真实场景中 App 是其他进程（浏览器页面、桌面 / 手机 App），连到 Hub 的 ``ws://<listen_addr>/app``。

运行：

    # 先生成绑定：bash bindings/uniffi/scripts/generate.sh && bash bindings/hub-uniffi/scripts/generate.sh
    cd sdks/python
    PYTHONPATH=src python examples/hub_llm_loop.py "帮我记一下明天买牛奶，然后把笔记清空"

设置了 ANTHROPIC_API_KEY（或传 ``--claude``，使用 ``ant auth login`` 的凭据）且装有 ``anthropic`` 包时
调用 Claude；否则用一个脚本化的"假模型"演示同样的循环。
"""

from __future__ import annotations

import asyncio
import sys
from typing import Any

from app_mcp import AppMcp
from app_mcp.hub import ApprovalRequest, Hub

MODEL = "claude-opus-5-5"
SESSION = "demo-conversation-1"  # 每段对话一个会话：首次接触 App 时结果里附带其总览


# ---------------------------------------------------------------------------
# 演示 App（真实场景中在别的进程里）
# ---------------------------------------------------------------------------

def start_demo_app(hub: Hub) -> AppMcp:
    notes: list[str] = []
    app = AppMcp("notes", "笔记", host_url=f"ws://{hub.listen_addr}/app", overview="记事本：添加、列出、清空笔记")

    @app.tool("add", description="添加一条笔记", risk="write")
    def add(text: str) -> dict:
        notes.append(text)
        return {"count": len(notes)}

    @app.tool("list", description="列出全部笔记", risk="read")
    def list_notes() -> list[str]:
        return list(notes)

    @app.tool("clear", description="清空全部笔记", risk="destructive")
    def clear() -> dict:
        notes.clear()
        return {"cleared": True}

    app.start()
    return app


# ---------------------------------------------------------------------------
# 审批：厂商自己的 UI（这里用终端提示）
# ---------------------------------------------------------------------------

def approve_in_terminal(req: ApprovalRequest) -> bool:
    """同步回调在线程池中执行，可以阻塞等待用户输入。"""
    if not sys.stdin.isatty():
        print(f"[审批] {req.app_name} / {req.tool}（风险 {req.risk.name}）→ 非交互环境，自动拒绝")
        return False
    answer = input(f"[审批] 允许 {req.app_name} 执行「{req.description}」？参数 {req.arguments_json} [y/N] ")
    return answer.strip().lower() in ("y", "yes")


# ---------------------------------------------------------------------------
# 模型：Claude（anthropic SDK）或脚本化假模型
# ---------------------------------------------------------------------------

class ClaudeModel:
    def __init__(self) -> None:
        import anthropic

        self._client = anthropic.AsyncAnthropic()

    async def turn(self, messages: list[dict], tools: list[dict]) -> tuple[list[dict], str]:
        """返回 (assistant content blocks, stop_reason)。"""
        # 默认开启服务端 fallback：主模型因策略拒绝时由服务端改用备用模型重跑同一请求。
        response = await self._client.beta.messages.create(
            model=MODEL,
            max_tokens=16000,
            betas=["server-side-fallback-2026-07-01"],
            fallbacks="default",
            system="你是桌面助手，可以调用本机 App 的工具完成用户的请求。",
            tools=tools,
            messages=messages,
        )
        return [b.model_dump(exclude_none=True) for b in response.content], response.stop_reason or ""


class ScriptedModel:
    """没有 anthropic 包 / 凭据时的演示模型：按固定剧本发出工具调用。"""

    def __init__(self) -> None:
        self._step = 0

    async def turn(self, messages: list[dict], tools: list[dict]) -> tuple[list[dict], str]:
        names = {t["name"] for t in tools}
        script: list[list[dict[str, Any]]] = [
            [{"type": "tool_use", "id": "toolu_1", "name": "notes__add", "input": {"text": "明天买牛奶"}}],
            [{"type": "tool_use", "id": "toolu_2", "name": "notes__clear", "input": {}}],
            [{"type": "tool_use", "id": "toolu_3", "name": "notes__list", "input": {}}],
        ]
        if self._step < len(script) and all(b["name"] in names for b in script[self._step]):
            blocks = script[self._step]
            self._step += 1
            return blocks, "tool_use"
        return [{"type": "text", "text": "好的：已记下「明天买牛奶」；清空笔记需要你的确认，已取消。"}], "end_turn"


def make_model(use_claude: bool) -> Any:
    if use_claude:
        try:
            return ClaudeModel()
        except ImportError as e:
            print(f"（没有 anthropic 包：{e}；改用脚本化演示模型）")
    return ScriptedModel()


# ---------------------------------------------------------------------------
# 自有 LLM 循环
# ---------------------------------------------------------------------------

async def run(user_input: str, use_claude: bool) -> None:
    with Hub(listen="127.0.0.1:0", enable_ipc=False, approval_min_risk="destructive") as hub:
        hub.set_approval_handler(approve_in_terminal)
        events = hub.events()
        app = start_demo_app(hub)
        try:
            await events.wait_for(lambda e: e.is_app_connected(), timeout=10)
            await asyncio.sleep(0.2)  # 等工具注册到达（真实场景中监听 TOOLS_CHANGED 刷新即可）

            model = make_model(use_claude)
            messages: list[dict] = [{"role": "user", "content": user_input}]
            for _ in range(10):  # 轮数上限
                # 每轮重新导出：App 的工具会随界面状态变化（TOOLS_CHANGED）。
                tools = hub.export_tools("anthropic", only_available=True)
                content, stop_reason = await model.turn(messages, tools)
                messages.append({"role": "assistant", "content": content})

                if stop_reason == "refusal":
                    print("模型拒绝了该请求。")
                    break
                tool_uses = [b for b in content if b.get("type") == "tool_use"]
                if stop_reason != "tool_use" or not tool_uses:
                    print("助手：", "".join(b.get("text", "") for b in content if b.get("type") == "text"))
                    break

                # 并发执行本轮全部工具调用；dispatch 直接返回 tool_result 块（失败时 is_error=true）。
                results = await asyncio.gather(*(hub.dispatch("anthropic", b, session=SESSION) for b in tool_uses))
                for b, r in zip(tool_uses, results):
                    print(f"  {b['name']}({b.get('input')}) → {'错误' if r.get('is_error') else '成功'}")
                messages.append({"role": "user", "content": results})
        finally:
            events.close()
            app.close()


if __name__ == "__main__":
    import os

    args = [a for a in sys.argv[1:] if a != "--claude"]
    use_claude = "--claude" in sys.argv or bool(
        os.environ.get("ANTHROPIC_API_KEY") or os.environ.get("ANTHROPIC_AUTH_TOKEN")
    )
    asyncio.run(run(args[0] if args else "帮我记一下明天买牛奶，然后把笔记清空", use_claude))
