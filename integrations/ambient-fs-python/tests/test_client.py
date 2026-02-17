"""Tests for client with mocked socket."""

import json
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

from ambient_fs import (
    AmbientFsClient,
    DaemonError,
    EventFilter,
    FileAwareness,
    FileEvent,
    Source,
    TreeNode,
    WatchResult,
)
from ambient_fs.transport import JsonRpcClient


@pytest.fixture
def mock_transport():
    """Create a mock transport client."""
    transport = AsyncMock(spec=JsonRpcClient)
    return transport


@pytest.fixture
def client(mock_transport):
    """Create a client with mocked transport."""
    return AmbientFsClient(_transport=mock_transport)


@pytest.mark.asyncio
async def test_watch_project(client, mock_transport):
    mock_transport.call.return_value = {
        "project_id": "proj-123",
        "watched_path": "/home/user/project",
    }

    result = await client.watch_project("/home/user/project")

    assert result.project_id == "proj-123"
    assert result.watched_path == "/home/user/project"
    mock_transport.call.assert_called_once_with(
        "watch_project",
        {"path": "/home/user/project"},
    )


@pytest.mark.asyncio
async def test_watch_project_legacy_string_response(client, mock_transport):
    """Test legacy response format where daemon returns just project_id string."""
    mock_transport.call.return_value = "proj-legacy-456"

    result = await client.watch_project("/home/user/project")

    assert result.project_id == "proj-legacy-456"


@pytest.mark.asyncio
async def test_unwatch_project(client, mock_transport):
    mock_transport.call.return_value = None

    await client.unwatch_project("proj-123")

    mock_transport.call.assert_called_once_with(
        "unwatch_project",
        {"project_id": "proj-123"},
    )


@pytest.mark.asyncio
async def test_query_events_no_filter(client, mock_transport):
    mock_transport.call.return_value = [
        {
            "timestamp": "2024-02-16T10:32:00Z",
            "event_type": "created",
            "file_path": "src/main.rs",
            "project_id": "proj-123",
            "source": "ai_agent",
            "source_id": "chat_42",
            "machine_id": "machine-1",
            "content_hash": "abc123",
        },
        {
            "timestamp": "2024-02-16T10:33:00Z",
            "event_type": "modified",
            "file_path": "src/lib.rs",
            "project_id": "proj-123",
            "source": "user",
            "machine_id": "machine-1",
        },
    ]

    events = await client.query_events()

    assert len(events) == 2
    assert events[0].file_path == "src/main.rs"
    assert events[0].source == Source.AI_AGENT
    assert events[0].source_id == "chat_42"
    assert events[1].event_type.value == "modified"
    assert events[1].source == Source.USER

    mock_transport.call.assert_called_once()


@pytest.mark.asyncio
async def test_query_events_with_filter(client, mock_transport):
    mock_transport.call.return_value = []

    filter_obj = EventFilter(
        project_id="proj-123",
        since=1708100000,
        source="ai_agent",
        limit=100,
    )

    await client.query_events(filter_obj)

    call_args = mock_transport.call.call_args
    assert call_args[0][0] == "query_events"
    params = call_args[0][1]
    assert params["project_id"] == "proj-123"
    assert params["since"] == 1708100000
    assert params["source"] == "ai_agent"
    assert params["limit"] == 100


@pytest.mark.asyncio
async def test_query_awareness(client, mock_transport):
    mock_transport.call.return_value = {
        "file_path": "src/main.rs",
        "project_id": "proj-123",
        "last_modified": "2024-02-16T10:32:00Z",
        "modified_by": "ai_agent",
        "modified_by_label": "Chat 42",
        "active_agent": "claude",
        "chat_references": 3,
        "todo_count": 1,
        "lint_hints": 0,
        "line_count": 100,
        "change_frequency": "hot",
    }

    awareness = await client.query_awareness("proj-123", "src/main.rs")

    assert awareness.file_path == "src/main.rs"
    assert awareness.modified_by == Source.AI_AGENT
    assert awareness.modified_by_label == "Chat 42"
    assert awareness.active_agent == "claude"
    assert awareness.line_count == 100
    assert awareness.change_frequency.value == "hot"


@pytest.mark.asyncio
async def test_query_tree(client, mock_transport):
    mock_transport.call.return_value = {
        "name": "root",
        "path": "",
        "is_dir": True,
        "children": [
            {
                "name": "README.md",
                "path": "README.md",
                "is_dir": False,
                "children": [],
            },
            {
                "name": "src",
                "path": "src",
                "is_dir": True,
                "children": [
                    {
                        "name": "main.rs",
                        "path": "src/main.rs",
                        "is_dir": False,
                        "children": [],
                    }
                ],
            },
        ],
    }

    tree = await client.query_tree("proj-123")

    assert tree.name == "root"
    assert tree.is_dir
    assert len(tree.children) == 2
    assert tree.children[0].name == "README.md"
    assert not tree.children[0].is_dir
    assert tree.children[1].name == "src"
    assert tree.children[1].is_dir
    assert tree.children[1].children[0].name == "main.rs"


@pytest.mark.asyncio
async def test_attribute_with_source_id(client, mock_transport):
    mock_transport.call.return_value = None

    await client.attribute(
        "proj-123",
        "src/auth.rs",
        "ai_agent",
        "chat-42",
    )

    call_args = mock_transport.call.call_args
    assert call_args[0][0] == "attribute"
    params = call_args[0][1]
    assert params["project_id"] == "proj-123"
    assert params["file_path"] == "src/auth.rs"
    assert params["source"] == "ai_agent"
    assert params["source_id"] == "chat-42"


@pytest.mark.asyncio
async def test_attribute_without_source_id(client, mock_transport):
    mock_transport.call.return_value = None

    await client.attribute(
        "proj-123",
        "src/auth.rs",
        "user",
    )

    call_args = mock_transport.call.call_args
    params = call_args[0][1]
    assert "source_id" not in params
    assert params["source"] == "user"


@pytest.mark.asyncio
async def test_query_agents(client, mock_transport):
    mock_transport.call.return_value = [
        {"id": "agent-1", "name": "claude", "status": "active"},
        {"id": "agent-2", "name": "cursor", "status": "idle"},
    ]

    agents = await client.query_agents()

    assert len(agents) == 2
    assert agents[0]["name"] == "claude"
    assert agents[1]["status"] == "idle"


@pytest.mark.asyncio
async def test_daemon_error_is_raised(client, mock_transport):
    """Test that DaemonError is raised when daemon returns an error."""
    mock_transport.call.side_effect = DaemonError(-1001, "project not found")

    with pytest.raises(DaemonError) as exc_info:
        await client.watch_project("/nonexistent")

    assert exc_info.value.code == -1001
    assert "project not found" in str(exc_info.value)


@pytest.mark.asyncio
async def test_close(client, mock_transport):
    await client.close()

    mock_transport.close.assert_called_once()


@pytest.mark.asyncio
async def test_context_manager(client, mock_transport):
    async with await AmbientFsClient.connect() as ctx_client:
        assert isinstance(ctx_client, AmbientFsClient)

    # Transport.close is called on exit
    # (but we can't easily mock the connect factory)


@pytest.mark.asyncio
async def test_subscribe_returns_unsubscribe(client):
    """Test that subscribe returns an unsubscribe callable."""
    unsub = client.subscribe("proj-123", lambda n: None)

    # Should return a callable
    assert callable(unsub)

    # Calling it should not raise
    unsub()


def test_socket_path_from_env():
    """Test that socket path is read from env var."""
    import os

    original = os.getenv("AMBIENT_FS_SOCK")
    try:
        os.environ["AMBIENT_FS_SOCK"] = "/custom/path.sock"
        from ambient_fs.transport import _get_socket_path

        path = _get_socket_path(None)
        assert path == "/custom/path.sock"
    finally:
        if original is None:
            os.environ.pop("AMBIENT_FS_SOCK", None)
        else:
            os.environ["AMBIENT_FS_SOCK"] = original


def test_socket_path_default():
    """Test default socket path when no env var."""
    import os

    original = os.getenv("AMBIENT_FS_SOCK")
    try:
        os.environ.pop("AMBIENT_FS_SOCK", None)
        from ambient_fs.transport import _get_socket_path

        path = _get_socket_path(None)
        assert path == "/tmp/ambient-fs.sock"
    finally:
        if original:
            os.environ["AMBIENT_FS_SOCK"] = original


def test_socket_path_explicit():
    """Test explicit socket path overrides env."""
    from ambient_fs.transport import _get_socket_path

    path = _get_socket_path("/my/path.sock")
    assert path == "/my/path.sock"
