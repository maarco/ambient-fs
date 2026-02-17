python client implementation
============================

status: design
created: 2026-02-16
affects: integrations/ambient-fs-python/


overview
--------

python client library for ambient-fs daemon. speaks the same
unix socket json-rpc 2.0 protocol as the rust client, but
feels pythonic (asyncio, dataclasses, type hints).

location: integrations/ambient-fs-python/
package name: ambient-fs (pypi)
python version: 3.10+

no external dependencies beyond stdlib. asyncio, json, dataclasses,
pathlib, typing. that's it.


package structure
-----------------

pyproject.toml              modern python build (pep 517)
src/ambient_fs/__init__.py  public api exports
src/ambient_fs/client.py    AmbientFsClient (async)
src/ambient_fs/sync.py      AmbientFsSyncClient (thread wrapper)
src/ambient_fs/types.py     dataclasses for protocol types
src/ambient_fs/errors.py    exception hierarchy
src/ambient_fs/_transport.py low-level socket + json-rpc
src/ambient_fs/py.typed     marker file for type checkers
tests/test_client.py        pytest with mock socket
tests/test_types.py         dataclass serialization tests
README.md                   usage examples
LICENSE                     mit


pyproject.toml
--------------

[build-system]
requires = ["hatchling"]
build-backend = "hatchling.build"

[project]
name = "ambient-fs"
version = "0.1.0"
description = "Python client for ambient-fs filesystem awareness daemon"
readme = "README.md"
requires-python = ">=3.10"
license = {text = "MIT"}
dependencies = []

[project.optional-dependencies]
dev = ["pytest", "pytest-asyncio", "mypy"]

[tool.hatch.build.targets.wheel]
packages = ["src/ambient_fs"]


python types (types.py)
-----------------------

all dataclasses match rust core types exactly. json serde
compatible with the daemon.

@dataclass
class FileEvent:
    timestamp: str              # iso8601 utc
    event_type: EventType
    file_path: str
    project_id: str
    source: Source
    source_id: str | None = None
    machine_id: str = ""
    content_hash: str | None = None
    old_path: str | None = None

class EventType(str, Enum):
    CREATED = "created"
    MODIFIED = "modified"
    DELETED = "deleted"
    RENAMED = "renamed"

class Source(str, Enum):
    USER = "user"
    AI_AGENT = "ai_agent"
    GIT = "git"
    BUILD = "build"
    VOICE = "voice"

@dataclass
class FileAwareness:
    file_path: str
    project_id: str
    last_modified: str         # iso8601 utc
    modified_by: Source
    modified_by_label: str | None = None
    active_agent: str | None = None
    chat_references: int = 0
    todo_count: int = 0
    lint_hints: int = 0
    line_count: int = 0
    change_frequency: ChangeFrequency = ChangeFrequency.COLD

class ChangeFrequency(str, Enum):
    HOT = "hot"
    WARM = "warm"
    COLD = "cold"

@dataclass
class TreeNode:
    name: str
    path: str
    is_dir: bool
    children: list[TreeNode] = field(default_factory=list)

@dataclass
class EventFilter:
    project_id: str | None = None
    since: int | None = None          # unix timestamp
    source: str | None = None
    limit: int | None = None

@dataclass
class WatchResult:
    project_id: str
    watched_path: str


errors.py
--------

class AmbientFsError(Exception):
    base exception for all client errors

class ConnectionError(AmbientFsError):
    failed to connect to daemon socket

class TimeoutError(AmbientFsError):
    request timed out

class DaemonError(AmbientFsError):
    daemon returned an error response

class InvalidResponseError(AmbientFsError):
    couldn't parse daemon response


AmbientFsClient (client.py)
----------------------------

async context manager. connects once, sends requests,
receives responses. newline-delimited json-rpc over
unix stream socket.

class AmbientFsClient:
    async def connect(
        socket_path: str | Path | None = None,
    ) -> AmbientFsClient:
        """
        classmethod to connect. socket_path defaults to:
        1. AMBIENT_FS_SOCK env var
        2. /tmp/ambient-fs.sock
        """
        ...

    async def watch_project(
        self, path: str | Path
    ) -> WatchResult:
        """
        watch a directory, returns project_id.
        daemon starts watching immediately.
        """
        ...

    async def unwatch_project(
        self, project_id: str
    ) -> None:
        """stop watching a project"""
        ...

    async def query_events(
        self, filter: EventFilter | None = None
    ) -> list[FileEvent]:
        """query event log with optional filter"""
        ...

    async def query_awareness(
        self, project_id: str, path: str | Path
    ) -> FileAwareness:
        """get awareness state for a file"""
        ...

    async def query_tree(
        self, project_id: str
    ) -> TreeNode:
        """get full file tree for a project"""
        ...

    async def attribute(
        self,
        project_id: str,
        file_path: str | Path,
        source: str,
        source_id: str | None = None,
    ) -> None:
        """attribute a file change to a source"""
        ...

    def subscribe(
        self,
        project_id: str,
        callback: Callable[[Notification], None],
    ) -> Callable[[], None]:
        """
        subscribe to notifications for a project.
        spawns a background task that reads notifications
        and calls the callback.

        returns an unsubscribe callable (closes task).
        """
        ...

    async def close(self) -> None:
        """close the socket connection"""
        ...

    async def __aenter__(self) -> AmbientFsClient:
        ...

    async def __aexit__(
        self, exc_type, exc_val, exc_tb
    ) -> None:
        ...

default socket path resolution:
    os.getenv("AMBIENT_FS_SOCK", "/tmp/ambient-fs.sock")


_transport.py (internal)
------------------------

low-level json-rpc protocol. not part of public api.

class _JsonRpcClient:
    async def _send_request(
        self, method: str, params: dict
    ) -> dict:
        """
        send json-rpc request, read response line.
        increments request id internally.
        """
        ...

    async def _read_line(self) -> str:
        """read until \n, handle partial reads"""
        ...

request format (same as rust client):
    {"jsonrpc":"2.0","method":"watch_project","params":{"path":"..."},"id":1}

response format:
    {"jsonrpc":"2.0","result":...,"id":1}
    or
    {"jsonrpc":"2.0","error":{"code":-32000,"message":"..."},"id":1}


sync.py
-----

thread wrapper for non-async code. runs an asyncio event
loop in a background thread.

class AmbientFsSyncClient:
    def __new__(
        cls, socket_path: str | Path | None = None
    ) -> AmbientFsSyncClient:
        """spawns event loop thread, connects"""
        ...

    def watch_project(self, path: str | Path) -> WatchResult:
        """blocking call, runs async in thread"""
        ...

    def query_events(
        self, filter: EventFilter | None = None
    ) -> list[FileEvent]:
        ...

    # ... all other client methods as blocking ...

    def close(self) -> None:
        """stop event loop thread"""
        ...

    def __enter__(self) -> AmbientFsSyncClient:
        ...

    def __exit__(self, exc_type, exc_val, exc_tb):
        ...

    def subscribe(
        self,
        project_id: str,
        callback: Callable[[Notification], None],
    ) -> Callable[[], None]:
        """
        same api as async client. callback runs in background
        thread, must be thread-safe.
        """
        ...


notification types
------------------

@dataclass
class Notification:
    project_id: str
    payload: NotificationPayload

@dataclass
class FileEventNotification:
    event: FileEvent

@dataclass
class AwarenessChangedNotification:
    path: str
    awareness: FileAwareness

NotificationPayload = FileEventNotification | AwarenessChangedNotification


usage examples
--------------

async:

    import asyncio
    from ambient_fs import AmbientFsClient, EventFilter

    async def main():
        async with await AmbientFsClient.connect() as client:
            # watch a project
            result = await client.watch_project("/home/user/project")
            print(f"watching: {result.project_id}")

            # query events
            events = await client.query_events(
                EventFilter(project_id=result.project_id, limit=10)
            )
            for e in events:
                print(f"{e.timestamp}: {e.event_type} {e.file_path}")

            # get awareness
            aw = await client.query_awareness(result.project_id, "src/main.rs")
            print(f"{aw.file_path}: {aw.change_frequency}, {aw.line_count} lines")

            # subscribe
            def on_notification(notif):
                print(f"got: {notif}")

            unsubscribe = client.subscribe(result.project_id, on_notification)
            await asyncio.sleep(60)  # listen for a minute
            unsubscribe()

    asyncio.run(main())

sync:

    from ambient_fs import AmbientFsSyncClient

    with AmbientFsSyncClient() as client:
        result = client.watch_project(".")
        events = client.query_events()
        for e in events[:5]:
            print(f"{e.file_path}: {e.event_type}")


test strategy
--------------

unit tests (no daemon):
    - dataclass roundtrip serialization
    - event filter serialization
    - json-rpc request construction
    - error handling paths

integration tests (requires running daemon):
    - connect + watch_project roundtrip
    - query_events returns list of fileevent
    - query_awareness returns fileawareness
    - attribute + query confirms source
    - subscribe receives notifications

use pytest with pytest-asyncio. mock socket with
asynciounixsocket or create temp unix socket with
a simple echo server for protocol tests.


type checking
-------------

mypy strict mode. all types exported in __init__.py:

    from ambient_fs import (
        AmbientFsClient,
        AmbientFsSyncClient,
        FileEvent,
        FileAwareness,
        EventType,
        Source,
        ChangeFrequency,
        TreeNode,
        EventFilter,
        WatchResult,
        AmbientFsError,
        ConnectionError,
        TimeoutError,
        DaemonError,
    )


differences from rust client
-----------------------------

- no builder pattern (use connect() classmethod)
- subscribe() takes callback instead of returning stream
  (stream is harder in python without rust's ownership)
- datetime parsed as str (iso8601) to avoid dateutil dep
- no agent query yet (agent_info type not finalized)
- sync client is thread wrapper, not separate implementation
