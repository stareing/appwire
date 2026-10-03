"""Hub SDK：Agent 登记（spec/hub-api.md 3.6「Agent 身份」）——``Hub(agents=...)``、:meth:`Hub.set_agents`。

需要 ``bindings/hub-uniffi/scripts/generate.sh`` 的生成物。
"""

from __future__ import annotations

import json
import urllib.request

import pytest

pytest.importorskip("app_mcp.hub", reason="没有 app_mcp_hub 生成物")

from app_mcp.hub import AgentCredential, Hub, HubError  # noqa: E402

pytestmark = pytest.mark.hub

CLAUDE = "claude-0123456789abcdef0123456789abcdef"
CURSOR = "cursor-0123456789abcdef0123456789abcdef"


def begin_task(hub: Hub, token: str) -> str:
    """经 ``/mcp`` 以 ``token`` 发无会话（2026-07-28）``apps.task.begin`` → 任务 ID。"""
    meta = {
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "t", "version": "1"},
    }
    body = {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "apps.task.begin", "arguments": {}, "_meta": meta}}
    req = urllib.request.Request(
        f"http://{hub.listen_addr}/mcp",
        data=json.dumps(body).encode(),
        method="POST",
        headers={
            "content-type": "application/json",
            "accept": "application/json, text/event-stream",
            "authorization": f"Bearer {token}",
            "mcp-protocol-version": "2026-07-28",
            "mcp-method": "tools/call",
            "mcp-name": "apps.task.begin",
        },
    )
    with urllib.request.urlopen(req, timeout=10) as resp:
        text = resp.read().decode()
    line = next(x for x in (ln.removeprefix("data:").strip() for ln in text.splitlines()) if x.startswith("{"))
    return json.loads(line)["result"]["structuredContent"]["taskId"]


def task_agent(hub: Hub, task_id: str) -> str | None:
    return next((t.agent for t in hub.status().tasks or [] if t.id == task_id), None)


def test_agents_config_and_set_agents() -> None:
    with pytest.raises(HubError.InvalidConfig) as e:
        Hub(enable_listen=False, enable_ipc=False, agents=[{"name": "a b", "token": CLAUDE}])
    assert CLAUDE not in str(e.value)
    with pytest.raises(ValueError):
        Hub(enable_listen=False, enable_ipc=False, agents=[{"name": "a", "tokn": CLAUDE}])

    with Hub(listen="127.0.0.1:0", mcp_http=True, enable_ipc=False, agents=[{"name": "claude", "token": CLAUDE}]) as hub:
        assert hub.status().agents == ["claude"]
        assert task_agent(hub, begin_task(hub, CLAUDE)) == "claude"

        with pytest.raises(HubError.Tool) as err:
            hub.set_agents([{"name": "a", "token": CLAUDE}, {"name": "a", "token": CURSOR}])
        assert err.value.kind == "INVALID_INPUT"
        assert hub.status().agents == ["claude"]

        hub.set_agents([AgentCredential(name="cursor", token=CURSOR)])
        assert hub.status().agents == ["cursor"]
        assert task_agent(hub, begin_task(hub, CURSOR)) == "cursor"
        hub.set_agents([])
        assert hub.status().agents == []
