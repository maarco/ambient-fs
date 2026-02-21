# Python Client Agent Activity Methods - Implementation Spec

## Overview

Add missing agent-related methods to the Python async client to enable agent
activity tracking and directory watching for `.agents/` JSONL files.

Reference implementations:
- `crates/ambient-fs-server/src/socket.rs` - Server handlers
- `crates/ambient-fs-server/src/agents.rs` - AgentActivity type definition

## Current State

The Python client (`integrations/ambient-fs-python/src/ambient_fs/client.py`) has:
- `watch_project(path) -> WatchResult`
- `unwatch_project(project_id) -> None`
- `query_agents() -> list[dict]` - Returns raw dicts, untyped

Missing:
- `watch_agents(path) -> None`
- `unwatch_agents(path) -> None`
- `report_agent_activity(activity) -> None`
- `AgentActivity` dataclass
- Proper `query_agents()` return type

## Server Protocol Reference

### watch_agents
```json
// Request
{"jsonrpc":"2.0","method":"watch_agents","params":{"path":"/abs/path/to/.agents"},"id":1}

// Response
{"jsonrpc":"2.0","result":{"watching":true,"path":"/abs/path/to/.agents"},"id":1}
```

### unwatch_agents
```json
// Request
{"jsonrpc":"2.0","method":"unwatch_agents","params":{"path":"/abs/path/to/.agents"},"id":2}

// Response
{"jsonrpc":"2.0","result":true,"id":2}
```

### report_agent_activity
```json
// Request
{
  "jsonrpc":"2.0",
  "method":"report_agent_activity",
  "params": {
    "ts": 1708099200,
    "agent": "claude-7",
    "action": "edit",
    "file": "src/auth.rs",
    "project": "ambient-fs",
    "tool": "claude-code",
    "session": "session-42",
    "intent": "fixing auth bypass",
    "lines": [42, 67],
    "done": false
  },
  "id": 3
}

// Response
{"jsonrpc":"2.0","result":{"recorded":true},"id":3}
```

### query_agents (current response format)
```json
// Request
{"jsonrpc":"2.0","method":"query_agents","params":{},"id":4}

// Response (each agent has these fields)
{
  "jsonrpc":"2.0",
  "result": [
    {
      "agent_id": "claude-7",
      "files": ["src/auth.rs", "src/main.rs"],
      "last_seen": 1708099200,
      "intent": "fixing auth",
      "tool": "claude-code",
      "session": "session-42"
    }
  ],
  "id": 4
}
```

---

## 1. AgentActivity Dataclass

Add to `src/ambient_fs/types.py`:

```python
@dataclass
class AgentActivity:
    """Agent activity event for tracking AI agent file operations.

    Matches the Rust AgentActivity struct in ambient-fs-server.
    """

    ts: int
    """Unix timestamp (seconds)."""

    agent: str
    """Agent identifier (e.g., "claude-7", "cursor-agent")."""

    action: str
    """What the agent is doing (e.g., "edit", "read", "write")."""

    file: str
    """File path being acted on."""

    project: str | None = None
    """Optional project identifier."""

    tool: str | None = None
    """Which AI tool (claude-code, cursor, etc)."""

    session: str | None = None
    """Session/conversation ID."""

    intent: str | None = None
    """Human-readable description of what the agent is doing."""

    lines: list[int] | None = None
    """Line numbers being edited."""

    confidence: float | None = None
    """0.0-1.0 confidence level (optional)."""

    done: bool | None = None
    """True when agent is finished with this file."""

    def is_done(self) -> bool:
        """Returns True if this activity marks a file as done."""
        return self.done is True

    @classmethod
    def minimal(
        cls,
        ts: int,
        agent: str,
        action: str,
        file: str,
    ) -> "AgentActivity":
        """Create activity with only required fields."""
        return cls(ts=ts, agent=agent, action=action, file=file)
```

---

## 2. Client Methods

Add to `src/ambient_fs/client.py`:

### 2.1 watch_agents

```python
async def watch_agents(self, path: str | Path) -> None:
    """Watch a directory for agent activity JSONL files.

    Registers a directory to be monitored for agent activity logs.
    The daemon will tail .jsonl files in this directory.

    Args:
        path: Directory path to watch (e.g., "/path/to/.agents").

    Raises:
        DaemonError: If path is invalid or not a directory.
    """
    await self._transport.call("watch_agents", {"path": str(path)})
```

### 2.2 unwatch_agents

```python
async def unwatch_agents(self, path: str | Path) -> None:
    """Stop watching a directory for agent activity.

    Args:
        path: Directory path to stop watching.
    """
    await self._transport.call("unwatch_agents", {"path": str(path)})
```

### 2.3 report_agent_activity

```python
async def report_agent_activity(self, activity: AgentActivity) -> None:
    """Report agent activity for source attribution.

    Files modified while an agent is active will get source=AiAgent
    instead of User in the event log.

    Args:
        activity: AgentActivity event with timestamp, agent, action, file.

    Raises:
        DaemonError: If activity is invalid.
    """
    from dataclasses import asdict

    # Build params, excluding None values
    params: dict[str, Any] = {
        "ts": activity.ts,
        "agent": activity.agent,
        "action": activity.action,
        "file": activity.file,
    }

    # Add optional fields if present
    for key in ("project", "tool", "session", "intent", "lines", "confidence", "done"):
        value = getattr(activity, key, None)
        if value is not None:
            params[key] = value

    await self._transport.call("report_agent_activity", params)
```

### 2.4 Update query_agents return type

Change signature from:
```python
async def query_agents(self) -> list[dict[str, Any]]:
```

To:
```python
async def query_agents(self, file: str | None = None) -> list[AgentInfo]:
    """Query active agents.

    Args:
        file: Optional file path to filter by. Returns only agents
              working on this specific file.

    Returns:
        List of AgentInfo objects with agent_id, files, last_seen,
        intent, tool, and session.
    """
    params: dict[str, Any] = {}
    if file:
        params["file"] = str(file)

    result = await self._transport.call("query_agents", params or None)
    if not isinstance(result, list):
        raise InvalidResponseError("expected list from query_agents")

    return [_decode_agent_info(a) for a in result]
```

Add decoder:
```python
def _decode_agent_info(data: dict[str, Any]) -> AgentInfo:
    """Decode AgentInfo from daemon response."""
    return AgentInfo(
        agent_id=data["agent_id"],
        files=data.get("files", []),
        last_seen=data["last_seen"],
        intent=data.get("intent"),
        tool=data.get("tool"),
        session=data.get("session"),
    )
```

---

## 3. AgentInfo Dataclass

Add to `src/ambient_fs/types.py`:

```python
@dataclass
class AgentInfo:
    """Information about an active agent."""

    agent_id: str
    """Agent identifier."""

    files: list[str]
    """Currently active files."""

    last_seen: int
    """Unix timestamp of last activity."""

    intent: str | None = None
    """Current intent/description."""

    tool: str | None = None
    """Tool being used."""

    session: str | None = None
    """Session identifier."""
```

---

## 4. Export Updates

Update `src/ambient_fs/__init__.py`:

```python
# Types (add to existing imports)
from .types import (
    # ... existing ...
    AgentActivity,
    AgentInfo,
)

__all__ = [
    # ... existing ...
    # Agent types (add)
    "AgentActivity",
    "AgentInfo",
]
```

---

## 5. Tests

Add to `tests/test_client.py`:

### 5.1 watch_agents tests

```python
import pytest
from ambient_fs import AgentActivity

@pytest.mark.asyncio
async def test_watch_agents_calls_transport(ambient_client):
    """watch_agents should call transport with correct params."""
    await ambient_client.watch_agents("/path/to/.agents")
    ambient_client._transport.call.assert_called_once_with(
        "watch_agents",
        {"path": "/path/to/.agents"}
    )

@pytest.mark.asyncio
async def test_watch_agents_with_path_object(ambient_client):
    """watch_agents should accept Path objects."""
    from pathlib import Path
    await ambient_client.watch_agents(Path("/path/to/.agents"))
    ambient_client._transport.call.assert_called_once_with(
        "watch_agents",
        {"path": "/path/to/.agents"}
    )
```

### 5.2 unwatch_agents tests

```python
@pytest.mark.asyncio
async def test_unwatch_agents_calls_transport(ambient_client):
    """unwatch_agents should call transport with correct params."""
    await ambient_client.unwatch_agents("/path/to/.agents")
    ambient_client._transport.call.assert_called_once_with(
        "unwatch_agents",
        {"path": "/path/to/.agents"}
    )
```

### 5.3 report_agent_activity tests

```python
@pytest.mark.asyncio
async def test_report_agent_activity_minimal(ambient_client):
    """report_agent_activity should send minimal activity."""
    activity = AgentActivity.minimal(
        ts=1708099200,
        agent="claude-7",
        action="edit",
        file="src/auth.rs",
    )
    await ambient_client.report_agent_activity(activity)

    ambient_client._transport.call.assert_called_once_with(
        "report_agent_activity",
        {
            "ts": 1708099200,
            "agent": "claude-7",
            "action": "edit",
            "file": "src/auth.rs",
        }
    )

@pytest.mark.asyncio
async def test_report_agent_activity_with_optionals(ambient_client):
    """report_agent_activity should include optional fields when present."""
    activity = AgentActivity(
        ts=1708099200,
        agent="claude-7",
        action="edit",
        file="src/auth.rs",
        project="ambient-fs",
        tool="claude-code",
        session="session-42",
        intent="fixing auth",
        lines=[42, 67],
        done=False,
    )
    await ambient_client.report_agent_activity(activity)

    # call_args[0] = positional args: ("report_agent_activity", {params})
    call_args = ambient_client._transport.call.call_args[0]
    params = call_args[1]
    assert params["project"] == "ambient-fs"
    assert params["tool"] == "claude-code"
    assert params["lines"] == [42, 67]
    assert params["done"] is False
    # Ensure None fields are omitted
    assert "confidence" not in params

@pytest.mark.asyncio
async def test_report_agent_activity_with_lines(ambient_client):
    """report_agent_activity should serialize lines list."""
    activity = AgentActivity(
        ts=1708099200,
        agent="agent-1",
        action="edit",
        file="src/main.rs",
        lines=[10, 20, 30],
    )
    await ambient_client.report_agent_activity(activity)

    call_args = ambient_client._transport.call.call_args[0]
    params = call_args[1]
    assert params["lines"] == [10, 20, 30]
```

### 5.4 AgentActivity type tests

Add to `tests/test_types.py`:

```python
def test_agent_activity_minimal():
    """AgentActivity.minimal creates activity with required fields."""
    activity = AgentActivity.minimal(1708099200, "agent-1", "edit", "src/main.rs")
    assert activity.ts == 1708099200
    assert activity.agent == "agent-1"
    assert activity.action == "edit"
    assert activity.file == "src/main.rs"
    assert activity.project is None
    assert activity.done is None

def test_agent_activity_is_done():
    """AgentActivity.is_done checks the done flag."""
    activity = AgentActivity.minimal(1708099200, "agent-1", "edit", "src/main.rs")
    assert not activity.is_done()

    activity.done = True
    assert activity.is_done()

    activity.done = False
    assert not activity.is_done()

def test_agent_activity_with_all_fields():
    """AgentActivity serializes all fields."""
    activity = AgentActivity(
        ts=1708099200,
        agent="claude-7",
        action="edit",
        file="src/auth.rs",
        project="ambient-fs",
        tool="claude-code",
        session="session-42",
        intent="fixing auth",
        lines=[42, 67],
        confidence=0.95,
        done=False,
    )
    assert activity.confidence == 0.95
    assert activity.lines == [42, 67]

def test_agent_info_creation():
    """AgentInfo creates with all fields."""
    info = AgentInfo(
        agent_id="claude-7",
        files=["src/auth.rs", "src/main.rs"],
        last_seen=1708099200,
        intent="fixing auth",
        tool="claude-code",
        session="session-42",
    )
    assert info.agent_id == "claude-7"
    assert len(info.files) == 2
    assert info.last_seen == 1708099200
```

### 5.5 query_agents updated tests

```python
@pytest.mark.asyncio
async def test_query_agents_returns_typed_info(ambient_client):
    """query_agents should return AgentInfo objects."""
    ambient_client._transport.call.return_value = [
        {
            "agent_id": "claude-7",
            "files": ["src/auth.rs"],
            "last_seen": 1708099200,
            "intent": "fixing auth",
            "tool": "claude-code",
            "session": "session-42",
        }
    ]

    agents = await ambient_client.query_agents()

    assert len(agents) == 1
    assert isinstance(agents[0], AgentInfo)
    assert agents[0].agent_id == "claude-7"
    assert agents[0].files == ["src/auth.rs"]
    assert agents[0].intent == "fixing auth"

@pytest.mark.asyncio
async def test_query_agents_with_file_filter(ambient_client):
    """query_agents should filter by file when specified."""
    ambient_client._transport.call.return_value = []

    await ambient_client.query_agents(file="src/auth.rs")

    ambient_client._transport.call.assert_called_once_with(
        "query_agents",
        {"file": "src/auth.rs"}
    )
```

---

## 6. Sync Client Updates

Update `src/ambient_fs/sync_client.py` to mirror the async methods:

```python
def watch_agents(self, path: str | Path) -> None:
    """Sync version of watch_agents."""
    self._run(self._client.watch_agents(path))

def unwatch_agents(self, path: str | Path) -> None:
    """Sync version of unwatch_agents."""
    self._run(self._client.unwatch_agents(path))

def report_agent_activity(self, activity: AgentActivity) -> None:
    """Sync version of report_agent_activity."""
    self._run(self._client.report_agent_activity(activity))
```

---

## Implementation Checklist

- [ ] Add `AgentActivity` dataclass to `types.py`
- [ ] Add `AgentInfo` dataclass to `types.py`
- [ ] Add `watch_agents()` to `client.py`
- [ ] Add `unwatch_agents()` to `client.py`
- [ ] Add `report_agent_activity()` to `client.py`
- [ ] Add `_decode_agent_info()` to `client.py`
- [ ] Update `query_agents()` signature and return type
- [ ] Update `__init__.py` exports
- [ ] Add sync client methods
- [ ] Add tests for all new methods
- [ ] Add tests for `AgentActivity` type
- [ ] Add tests for `AgentInfo` type
- [ ] Run `pytest` and ensure all pass
