"""Low-level JSON-RPC 2.0 transport over Unix socket.

Internal module, not part of public API.
"""

import asyncio
import json
import os
from pathlib import Path
from typing import Any

from .errors import ConnectionError, DaemonError, InvalidResponseError

DEFAULT_SOCKET_PATH = "/tmp/ambient-fs.sock"


def _get_socket_path(path: str | Path | None) -> str:
    """Get socket path from arg, env, or default."""
    if path is None:
        path = os.getenv("AMBIENT_FS_SOCK", DEFAULT_SOCKET_PATH)
    return str(path)


class JsonRpcClient:
    """Low-level JSON-RPC client over Unix socket."""

    _next_id: int

    def __init__(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        self._reader = reader
        self._writer = writer
        self._next_id = 1
        self._lock = asyncio.Lock()

    @classmethod
    async def connect(cls, socket_path: str | Path | None = None) -> "JsonRpcClient":
        """Connect to the daemon socket."""
        path = _get_socket_path(socket_path)
        try:
            reader, writer = await asyncio.open_unix_connection(path)
        except (FileNotFoundError, OSError) as e:
            raise ConnectionError(f"failed to connect to {path}: {e}") from e
        client = cls(reader, writer)
        client._next_id = 1
        return client

    async def call(self, method: str, params: dict[str, Any] | None = None) -> Any:
        """Send a JSON-RPC request and return the result."""
        async with self._lock:
            request_id = self._next_id
            self._next_id += 1

            request = {
                "jsonrpc": "2.0",
                "method": method,
                "params": params or {},
                "id": request_id,
            }

            # Send request + newline
            request_json = json.dumps(request, separators=(",", ":"))
            self._writer.write(request_json.encode())
            self._writer.write(b"\n")
            await self._writer.drain()

            # Read response line
            response_line = await self._read_line()

            # Parse response
            try:
                response = json.loads(response_line)
            except json.JSONDecodeError as e:
                raise InvalidResponseError(f"invalid json: {response_line}") from e

            # Validate JSON-RPC response
            if response.get("jsonrpc") != "2.0":
                raise InvalidResponseError(f"missing jsonrpc version: {response_line}")

            if "error" in response:
                error = response["error"]
                raise DaemonError(error["code"], error["message"])

            if "result" not in response:
                raise InvalidResponseError(f"no result or error: {response_line}")

            return response["result"]

    async def _read_line(self) -> str:
        """Read a newline-terminated line, handling partial reads."""
        chunks: list[bytes] = []
        while True:
            try:
                chunk = await asyncio.wait_for(self._reader.readline(), timeout=30.0)
                if not chunk:
                    if chunks:
                        break
                    raise InvalidResponseError("connection closed by daemon")
                if b"\n" in chunk:
                    line_bytes = b"".join(chunks) + chunk
                    return line_bytes.decode()
                chunks.append(chunk)
            except asyncio.TimeoutError:
                raise TimeoutError("request timed out after 30 seconds")
        return b"".join(chunks).decode()

    async def close(self) -> None:
        """Close the connection."""
        try:
            self._writer.close()
            await self._writer.wait_closed()
        except Exception:
            pass

    async def __aenter__(self) -> "JsonRpcClient":
        return self

    async def __aexit__(self, *exc: Any) -> None:
        await self.close()
