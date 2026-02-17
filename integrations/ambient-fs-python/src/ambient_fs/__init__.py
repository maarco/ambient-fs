"""ambient-fs: Python client for filesystem awareness daemon.

Example:
    from ambient_fs import AmbientFsClient, EventFilter

    async with await AmbientFsClient.connect() as client:
        result = await client.watch_project(".")
        events = await client.query_events(
            EventFilter(project_id=result.project_id, limit=10)
        )
        for event in events:
            print(f"{event.file_path}: {event.event_type}")
"""

# Error types
from .errors import (
    AmbientFsError,
    ConnectionError,
    DaemonError,
    InvalidResponseError,
    TimeoutError,
)

# Async client
from .client import AmbientFsClient

# Sync client
from .sync_client import AmbientFsSyncClient

# Types
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

# Re-export EventType from types (where it's defined)
from .types import EventType

__all__ = [
    # Errors
    "AmbientFsError",
    "ConnectionError",
    "DaemonError",
    "InvalidResponseError",
    "TimeoutError",
    # Clients
    "AmbientFsClient",
    "AmbientFsSyncClient",
    # Types
    "EventType",
    "Source",
    "ChangeFrequency",
    "FileEvent",
    "FileAwareness",
    "TreeNode",
    "EventFilter",
    "WatchResult",
    "Notification",
    "FileEventNotification",
    "NotificationPayload",
]

__version__ = "0.1.0"
