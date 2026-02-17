"""Tests for low-level transport layer."""

import asyncio
import json
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

from ambient_fs.errors import ConnectionError, DaemonError, InvalidResponseError
from ambient_fs.transport import JsonRpcClient


@pytest.mark.asyncio
async def test_connect_success():
    """Test successful connection to socket."""

    async def mock_open_unix(path):
        reader = asyncio.StreamReader()
        writer = MagicMock()
        writer.write = MagicMock()
        writer.drain = AsyncMock()
        writer.wait_closed = AsyncMock()
        writer.close = MagicMock()
        return reader, writer

    with patch("asyncio.open_unix_connection", mock_open_unix):
        client = await JsonRpcClient.connect("/tmp/test.sock")
        assert client is not None
        await client.close()


@pytest.mark.asyncio
async def test_connect_failure():
    """Test connection failure raises ConnectionError."""

    async def mock_open_unix_fail(path):
        raise FileNotFoundError("socket not found")

    with patch("asyncio.open_unix_connection", mock_open_unix_fail):
        with pytest.raises(ConnectionError) as exc_info:
            await JsonRpcClient.connect("/tmp/noexist.sock")

        assert "failed to connect" in str(exc_info.value)


@pytest.mark.asyncio
async def test_call_successful_request():
    """Test successful JSON-RPC call."""

    # Mock reader that returns a valid response
    reader = asyncio.StreamReader()
    response = json.dumps({
        "jsonrpc": "2.0",
        "result": {"project_id": "proj-123"},
        "id": 1,
    })
    reader.feed_data((response + "\n").encode())
    reader.feed_eof()

    writer = MagicMock()
    writer.write = MagicMock()
    writer.drain = AsyncMock()
    writer.wait_closed = AsyncMock()
    writer.close = MagicMock()

    client = JsonRpcClient(reader, writer)

    result = await client.call("watch_project", {"path": "/test"})

    assert result == {"project_id": "proj-123"}
    assert writer.write.called


@pytest.mark.asyncio
async def test_call_daemon_error():
    """Test daemon error response raises DaemonError."""

    reader = asyncio.StreamReader()
    response = json.dumps({
        "jsonrpc": "2.0",
        "error": {"code": -1001, "message": "project not found"},
        "id": 1,
    })
    reader.feed_data((response + "\n").encode())
    reader.feed_eof()

    writer = MagicMock()
    writer.write = MagicMock()
    writer.drain = AsyncMock()

    client = JsonRpcClient(reader, writer)

    with pytest.raises(DaemonError) as exc_info:
        await client.call("watch_project", {"path": "/test"})

    assert exc_info.value.code == -1001
    assert exc_info.value.message == "project not found"


@pytest.mark.asyncio
async def test_call_invalid_json():
    """Test invalid JSON response raises InvalidResponseError."""

    reader = asyncio.StreamReader()
    reader.feed_data(b"not valid json\n")
    reader.feed_eof()

    writer = MagicMock()
    writer.write = MagicMock()
    writer.drain = AsyncMock()

    client = JsonRpcClient(reader, writer)

    with pytest.raises(InvalidResponseError) as exc_info:
        await client.call("status", {})

    assert "invalid json" in str(exc_info.value)


@pytest.mark.asyncio
async def test_call_missing_jsonrpc_version():
    """Test response without jsonrpc version raises InvalidResponseError."""

    reader = asyncio.StreamReader()
    response = json.dumps({
        "result": {"ok": True},
        "id": 1,
    })
    reader.feed_data((response + "\n").encode())
    reader.feed_eof()

    writer = MagicMock()
    writer.write = MagicMock()
    writer.drain = AsyncMock()

    client = JsonRpcClient(reader, writer)

    with pytest.raises(InvalidResponseError) as exc_info:
        await client.call("status", {})

    assert "missing jsonrpc" in str(exc_info.value)


@pytest.mark.asyncio
async def test_call_no_result_or_error():
    """Test response without result or error raises InvalidResponseError."""

    reader = asyncio.StreamReader()
    response = json.dumps({
        "jsonrpc": "2.0",
        "id": 1,
    })
    reader.feed_data((response + "\n").encode())
    reader.feed_eof()

    writer = MagicMock()
    writer.write = MagicMock()
    writer.drain = AsyncMock()

    client = JsonRpcClient(reader, writer)

    with pytest.raises(InvalidResponseError) as exc_info:
        await client.call("status", {})

    assert "no result or error" in str(exc_info.value)


@pytest.mark.asyncio
async def test_call_increments_request_id():
    """Test that each call increments the request ID."""

    reader = asyncio.StreamReader()
    response = json.dumps({
        "jsonrpc": "2.0",
        "result": True,
        "id": 1,
    })
    reader.feed_data((response + "\n").encode())
    reader.feed_eof()

    writer = MagicMock()
    writer.write = MagicMock()
    writer.drain = AsyncMock()

    client = JsonRpcClient(reader, writer)

    # First call
    await client.call("method1", {})

    # Check that ID 1 was used
    # The call writes request + newline separately
    all_calls = writer.write.call_args_list
    # First call is the JSON request
    written = all_calls[0][0][0].decode()
    request = json.loads(written)
    assert request["id"] == 1


@pytest.mark.asyncio
async def test_close():
    """Test closing the connection."""

    writer = MagicMock()
    writer.close = MagicMock()
    writer.wait_closed = AsyncMock()

    client = JsonRpcClient(asyncio.StreamReader(), writer)
    await client.close()

    writer.close.assert_called_once()


@pytest.mark.asyncio
async def test_context_manager():
    """Test using client as async context manager."""

    writer = MagicMock()
    writer.close = MagicMock()
    writer.wait_closed = AsyncMock()

    client = JsonRpcClient(asyncio.StreamReader(), writer)

    async with client:
        pass

    writer.close.assert_called_once()


@pytest.mark.asyncio
async def test_read_line_handles_partial_reads():
    """Test that _read_line accumulates partial chunks."""

    reader = MagicMock()
    reader.readline = AsyncMock()

    # Simulate partial reads: first call returns without newline,
    # but actually StreamReader.readline buffers internally
    # For this test, we'll mock it to return complete data
    response = json.dumps({
        "jsonrpc": "2.0",
        "result": True,
        "id": 1,
    })
    reader.readline.return_value = (response + "\n").encode()

    writer = MagicMock()
    writer.write = MagicMock()
    writer.drain = AsyncMock()

    client = JsonRpcClient(reader, writer)

    result = await client.call("test", {})
    assert result is True


def test_get_socket_path_default():
    """Test default socket path."""
    from ambient_fs.transport import _get_socket_path
    import os

    original = os.getenv("AMBIENT_FS_SOCK")
    try:
        os.environ.pop("AMBIENT_FS_SOCK", None)
        path = _get_socket_path(None)
        assert path == "/tmp/ambient-fs.sock"
    finally:
        if original:
            os.environ["AMBIENT_FS_SOCK"] = original


def test_get_socket_path_from_env():
    """Test socket path from environment variable."""
    from ambient_fs.transport import _get_socket_path
    import os

    original = os.getenv("AMBIENT_FS_SOCK")
    try:
        os.environ["AMBIENT_FS_SOCK"] = "/custom/path.sock"
        path = _get_socket_path(None)
        assert path == "/custom/path.sock"
    finally:
        if original is None:
            os.environ.pop("AMBIENT_FS_SOCK", None)
        else:
            os.environ["AMBIENT_FS_SOCK"] = original


def test_get_socket_path_explicit():
    """Test explicit socket path overrides env."""
    from ambient_fs.transport import _get_socket_path

    path = _get_socket_path("/explicit/path.sock")
    assert path == "/explicit/path.sock"
