"""Synchronous (blocking) wrapper around async client.

Runs an asyncio event loop in a background thread.
"""

import asyncio
import threading
from concurrent.futures import Future
from pathlib import Path
from typing import Any, Callable

from .client import AmbientFsClient
from .types import (
    ChangeFrequency,
    EventFilter,
    FileAwareness,
    FileEvent,
    Notification,
    Source,
    TreeNode,
    WatchResult,
)


class AmbientFsSyncClient:
    """Blocking wrapper for ambient-fs client.

    Runs an asyncio event loop in a background thread. All methods are
    blocking and thread-safe.

    Usage:
        with AmbientFsSyncClient() as client:
            result = client.watch_project("/path/to/project")
            events = client.query_events(
                EventFilter(project_id=result.project_id)
            )
    """

    def __init__(
        self, socket_path: str | Path | None = None
    ) -> None:
        self._socket_path = socket_path
        self._loop: asyncio.AbstractEventLoop | None = None
        self._thread: threading.Thread | None = None
        self._client: AmbientFsClient | None = None
        self._ready = threading.Event()
        self._lock = threading.Lock()

        # Start the event loop thread
        self._start()

    def _start(self) -> None:
        """Start the event loop in a background thread."""

        def run_loop() -> None:
            self._loop = asyncio.new_event_loop()
            asyncio.set_event_loop(self._loop)
            self._ready.set()
            self._loop.run_forever()

        self._thread = threading.Thread(target=run_loop, daemon=True)
        self._thread.start()
        self._ready.wait()

        # Connect in the event loop
        self._run_async(self._connect)

    async def _connect(self) -> None:
        self._client = await AmbientFsClient.connect(self._socket_path)

    def _run_async(self, coro: Any) -> Any:
        """Run a coroutine in the event loop and return the result."""
        if self._loop is None:
            raise RuntimeError("event loop not running")

        future: Future[Any] = Future()

        async def wrapper() -> None:
            try:
                result = await coro
                future.set_result(result)
            except Exception as e:
                future.set_exception(e)

        if self._loop is None:
            raise RuntimeError("event loop not running")
        self._loop.call_soon_threadsafe(
            lambda: self._loop.create_task(wrapper())
        )
        return future.result()

    def watch_project(self, path: str | Path) -> WatchResult:
        """Watch a directory path for events."""
        if self._client is None:
            raise RuntimeError("client not connected")
        return self._run_async(self._client.watch_project(path))

    def unwatch_project(self, project_id: str) -> None:
        """Stop watching a project."""
        if self._client is None:
            raise RuntimeError("client not connected")
        self._run_async(self._client.unwatch_project(project_id))

    def query_events(
        self, filter: EventFilter | None = None
    ) -> list[FileEvent]:
        """Query event log with optional filter."""
        if self._client is None:
            raise RuntimeError("client not connected")
        return self._run_async(self._client.query_events(filter))

    def query_awareness(
        self, project_id: str, path: str | Path
    ) -> FileAwareness:
        """Get awareness state for a file."""
        if self._client is None:
            raise RuntimeError("client not connected")
        return self._run_async(
            self._client.query_awareness(project_id, path)
        )

    def query_tree(self, project_id: str) -> TreeNode:
        """Get full file tree for a project."""
        if self._client is None:
            raise RuntimeError("client not connected")
        return self._run_async(self._client.query_tree(project_id))

    def attribute(
        self,
        project_id: str,
        file_path: str | Path,
        source: str,
        source_id: str | None = None,
    ) -> None:
        """Attribute a file change to a source."""
        if self._client is None:
            raise RuntimeError("client not connected")
        self._run_async(
            self._client.attribute(project_id, file_path, source, source_id)
        )

    def query_agents(self) -> list[dict[str, Any]]:
        """Query active agents."""
        if self._client is None:
            raise RuntimeError("client not connected")
        return self._run_async(self._client.query_agents())

    def subscribe(
        self,
        project_id: str,
        callback: Callable[[Notification], None],
    ) -> Callable[[], None]:
        """Subscribe to notifications for a project.

        The callback runs in the background thread. Must be thread-safe.
        """
        if self._client is None:
            raise RuntimeError("client not connected")
        return self._client.subscribe(project_id, callback)

    def close(self) -> None:
        """Close the connection and stop the event loop."""
        if self._loop is None:
            return

        def stop_loop() -> None:
            if self._client:
                asyncio.create_task(self._client.close())
            self._loop.stop()

        self._loop.call_soon_threadsafe(stop_loop)

        if self._thread:
            self._thread.join(timeout=5.0)
            self._thread = None

        self._loop = None
        self._client = None

    def __enter__(self) -> "AmbientFsSyncClient":
        return self

    def __exit__(self, *exc: Any) -> None:
        self.close()
