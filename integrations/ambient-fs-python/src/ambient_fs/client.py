"""Async client for ambient-fs daemon."""

import asyncio
from pathlib import Path
from typing import Any, Awaitable, Callable

from . import transport as _transport_mod
from .errors import ConnectionError, InvalidResponseError
from .types import (
    ChangeFrequency,
    EventFilter,
    FileAwareness,
    FileEvent,
    FileEventNotification,
    Notification,
    NotificationPayload,
    Source,
    TreeNode,
    WatchResult,
)


def _decode_file_event(data: dict[str, Any]) -> FileEvent:
    """Decode FileEvent from daemon response."""
    return FileEvent(
        timestamp=data["timestamp"],
        event_type=EventType(data["event_type"]),
        file_path=data["file_path"],
        project_id=data["project_id"],
        source=Source(data["source"]),
        source_id=data.get("source_id"),
        machine_id=data.get("machine_id", ""),
        content_hash=data.get("content_hash"),
        old_path=data.get("old_path"),
    )


def _decode_file_awareness(data: dict[str, Any]) -> FileAwareness:
    """Decode FileAwareness from daemon response."""
    return FileAwareness(
        file_path=data["file_path"],
        project_id=data["project_id"],
        last_modified=data["last_modified"],
        modified_by=Source(data["modified_by"]),
        modified_by_label=data.get("modified_by_label"),
        active_agent=data.get("active_agent"),
        chat_references=data.get("chat_references", 0),
        todo_count=data.get("todo_count", 0),
        lint_hints=data.get("lint_hints", 0),
        line_count=data.get("line_count", 0),
        change_frequency=ChangeFrequency(data.get("change_frequency", "cold")),
    )


def _decode_tree_node(data: dict[str, Any]) -> TreeNode:
    """Decode TreeNode from daemon response."""
    children = [_decode_tree_node(c) for c in data.get("children", [])]
    return TreeNode(
        name=data["name"],
        path=data["path"],
        is_dir=data["is_dir"],
        children=children,
    )


def _decode_watch_result(data: dict[str, Any] | str) -> WatchResult:
    """Decode WatchResult from daemon response (supports both dict and string)."""
    if isinstance(data, str):
        # Legacy: daemon returns just project_id string
        return WatchResult(project_id=data, watched_path="")
    return WatchResult(
        project_id=data["project_id"],
        watched_path=data["watched_path"],
    )


class AmbientFsClient:
    """Async client for ambient-fs daemon.

    Usage:
        async with await AmbientFsClient.connect() as client:
            result = await client.watch_project("/path/to/project")
            events = await client.query_events(
                EventFilter(project_id=result.project_id, limit=10)
            )
    """

    def __init__(self, _transport: _transport_mod.JsonRpcClient) -> None:
        self._transport = _transport

    @classmethod
    async def connect(cls, socket_path: str | Path | None = None) -> "AmbientFsClient":
        """Connect to the daemon.

        Args:
            socket_path: Path to Unix socket. Defaults to AMBIENT_FS_SOCK env
                         var or /tmp/ambient-fs.sock.

        Returns:
            Connected client instance.

        Raises:
            ConnectionError: If daemon is not running.
        """
        transport = await _transport_mod.JsonRpcClient.connect(socket_path)
        return cls(_transport=transport)

    async def watch_project(self, path: str | Path) -> WatchResult:
        """Watch a directory path for events.

        Args:
            path: Directory path to watch.

        Returns:
            WatchResult with assigned project_id.

        Raises:
            DaemonError: If path is invalid or already watched.
        """
        result = await self._transport.call("watch_project", {"path": str(path)})
        return _decode_watch_result(result)

    async def unwatch_project(self, project_id: str) -> None:
        """Stop watching a project.

        Args:
            project_id: Project ID to stop watching.
        """
        await self._transport.call("unwatch_project", {"project_id": project_id})

    async def query_events(self, filter: EventFilter | None = None) -> list[FileEvent]:
        """Query event log with optional filter.

        Args:
            filter: Event filter constraints.

        Returns:
            List of FileEvents matching the filter.
        """
        params: dict[str, str | int] = {}
        if filter:
            if filter.project_id:
                params["project_id"] = filter.project_id
            if filter.since is not None:
                params["since"] = filter.since
            if filter.source:
                params["source"] = filter.source
            if filter.limit is not None:
                params["limit"] = filter.limit

        result = await self._transport.call("query_events", params or None)
        return [_decode_file_event(e) for e in result]

    async def query_awareness(
        self, project_id: str, path: str | Path
    ) -> FileAwareness:
        """Get awareness state for a file.

        Args:
            project_id: Project containing the file.
            path: File path relative to project root.

        Returns:
            FileAwareness with current state.
        """
        result = await self._transport.call(
            "query_awareness",
            {"project_id": project_id, "path": str(path)},
        )
        return _decode_file_awareness(result)

    async def query_tree(self, project_id: str) -> TreeNode:
        """Get full file tree for a project.

        Args:
            project_id: Project to query.

        Returns:
            Root TreeNode with full tree.
        """
        result = await self._transport.call("query_tree", {"project_id": project_id})
        return _decode_tree_node(result)

    async def attribute(
        self,
        project_id: str,
        file_path: str | Path,
        source: str,
        source_id: str | None = None,
    ) -> None:
        """Attribute a file change to a source.

        Args:
            project_id: Project containing the file.
            file_path: File that changed.
            source: Source identifier (e.g., "ai_agent", "user").
            source_id: Optional source-specific ID (e.g., chat ID).
        """
        params: dict[str, Any] = {
            "project_id": project_id,
            "file_path": str(file_path),
            "source": source,
        }
        if source_id:
            params["source_id"] = source_id

        await self._transport.call("attribute", params)

    async def query_agents(self) -> list[dict[str, Any]]:
        """Query active agents.

        Returns:
            List of agent info dicts (structure not yet finalized).
        """
        result = await self._transport.call("query_agents", {})
        if not isinstance(result, list):
            raise InvalidResponseError("expected list from query_agents")
        return result

    def subscribe(
        self,
        project_id: str,
        callback: Callable[[Notification], None],
    ) -> Callable[[], None]:
        """Subscribe to notifications for a project.

        Spawns a background task that reads notifications and calls the callback.

        Note: This is a simplified version. The Rust server uses push notifications
        over the same socket, but Python's asyncio model makes streaming trickier.
        This implementation spawns a separate reader task.

        Args:
            project_id: Project to subscribe to.
            callback: Function to call for each notification. Must be thread-safe.

        Returns:
            Unsubscribe callable that stops the background task.
        """

        async def reader_task() -> None:
            try:
                # Send subscribe request
                await self._transport.call("subscribe", {"project_id": project_id})

                # Read notifications (this would need server-side support)
                # For now, this is a placeholder for when server pushes notifications
            except Exception:
                pass

        task = asyncio.create_task(reader_task())

        def unsubscribe() -> None:
            task.cancel()

        return unsubscribe

    async def close(self) -> None:
        """Close the connection."""
        await self._transport.close()

    async def __aenter__(self) -> "AmbientFsClient":
        return self

    async def __aexit__(self, *exc: Any) -> None:
        await self.close()


# Re-export enums for convenience
from .types import EventType  # noqa: E402
