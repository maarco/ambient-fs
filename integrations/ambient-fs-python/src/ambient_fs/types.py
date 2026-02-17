"""Data types matching ambient-fs Rust core types.

All dataclasses are JSON-compatible with the daemon protocol.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from typing import Self

# Re-export Self for older Python versions
try:
    from typing import Self  # type: ignore
except ImportError:
    from typing_extensions import Self  # type: ignore


class EventType(str, Enum):
    """Type of filesystem event."""

    CREATED = "created"
    MODIFIED = "modified"
    DELETED = "deleted"
    RENAMED = "renamed"


class Source(str, Enum):
    """Source of a filesystem change."""

    USER = "user"
    AI_AGENT = "ai_agent"
    GIT = "git"
    BUILD = "build"
    VOICE = "voice"


class ChangeFrequency(str, Enum):
    """How frequently a file has been changing."""

    HOT = "hot"      # Changed in the last 10 minutes
    WARM = "warm"    # Changed in the last hour
    COLD = "cold"    # Not changed recently


@dataclass
class FileEvent:
    """A filesystem event with full attribution."""

    timestamp: str
    event_type: EventType
    file_path: str
    project_id: str
    source: Source
    source_id: str | None = None
    machine_id: str = ""
    content_hash: str | None = None
    old_path: str | None = None

    def is_rename(self) -> bool:
        """Returns True if this event represents a rename."""
        return self.event_type == EventType.RENAMED and self.old_path is not None


@dataclass
class FileAwareness:
    """Aggregated awareness state for a single file."""

    file_path: str
    project_id: str
    last_modified: str
    modified_by: Source
    modified_by_label: str | None = None
    active_agent: str | None = None
    chat_references: int = 0
    todo_count: int = 0
    lint_hints: int = 0
    line_count: int = 0
    change_frequency: ChangeFrequency = ChangeFrequency.COLD


@dataclass
class TreeNode:
    """A node in the file tree."""

    name: str
    path: str
    is_dir: bool
    children: list[TreeNode] = field(default_factory=list)

    @classmethod
    def file(cls, name: str, path: str) -> Self:
        """Create a file node."""
        return cls(name=name, path=path, is_dir=False)

    @classmethod
    def dir(cls, name: str, path: str) -> Self:
        """Create a directory node."""
        return cls(name=name, path=path, is_dir=True)


@dataclass
class EventFilter:
    """Event filter for queries."""

    project_id: str | None = None
    since: int | None = None
    source: str | None = None
    limit: int | None = None


@dataclass
class WatchResult:
    """Result of watching a project."""

    project_id: str
    watched_path: str


@dataclass
class Notification:
    """Server push notification."""

    project_id: str
    payload: "NotificationPayload"


@dataclass
class FileEventNotification:
    """Notification containing a file event."""

    event: FileEvent


@dataclass
class AwarenessChangedNotification:
    """Notification indicating awareness changed for a file."""

    path: str
    awareness: FileAwareness


NotificationPayload = FileEventNotification | AwarenessChangedNotification
