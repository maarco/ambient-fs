"""Tests for data types and serialization."""

import json

from ambient_fs.types import (
    ChangeFrequency,
    EventFilter,
    FileAwareness,
    FileEvent,
    Notification,
    FileEventNotification,
    Source,
    TreeNode,
    EventType,
)


def test_event_type_enum() -> None:
    assert EventType.CREATED.value == "created"
    assert EventType.MODIFIED.value == "modified"
    assert EventType.DELETED.value == "deleted"
    assert EventType.RENAMED.value == "renamed"


def test_source_enum() -> None:
    assert Source.USER.value == "user"
    assert Source.AI_AGENT.value == "ai_agent"
    assert Source.GIT.value == "git"
    assert Source.BUILD.value == "build"
    assert Source.VOICE.value == "voice"


def test_change_frequency_enum() -> None:
    assert ChangeFrequency.HOT.value == "hot"
    assert ChangeFrequency.WARM.value == "warm"
    assert ChangeFrequency.COLD.value == "cold"


def test_file_event_creation() -> None:
    event = FileEvent(
        timestamp="2024-02-16T10:32:00Z",
        event_type=EventType.CREATED,
        file_path="src/main.rs",
        project_id="proj-123",
        source=Source.AI_AGENT,
        source_id="chat_42",
        machine_id="machine-1",
        content_hash="abc123",
    )

    assert event.file_path == "src/main.rs"
    assert event.event_type == EventType.CREATED
    assert event.source == Source.AI_AGENT
    assert event.source_id == "chat_42"
    assert not event.is_rename()


def test_file_event_rename() -> None:
    event = FileEvent(
        timestamp="2024-02-16T10:32:00Z",
        event_type=EventType.RENAMED,
        file_path="src/new.rs",
        project_id="proj-123",
        source=Source.USER,
        machine_id="m1",
        old_path="src/old.rs",
    )

    assert event.is_rename()
    assert event.old_path == "src/old.rs"


def test_file_event_rename_without_old_path() -> None:
    event = FileEvent(
        timestamp="2024-02-16T10:32:00Z",
        event_type=EventType.RENAMED,
        file_path="src/new.rs",
        project_id="proj-123",
        source=Source.USER,
        machine_id="m1",
    )

    assert not event.is_rename()


def test_file_awareness_creation() -> None:
    awareness = FileAwareness(
        file_path="src/main.rs",
        project_id="proj-123",
        last_modified="2024-02-16T10:32:00Z",
        modified_by=Source.AI_AGENT,
        modified_by_label="Chat 42",
        active_agent="claude",
        chat_references=3,
        todo_count=1,
        lint_hints=0,
        line_count=100,
        change_frequency=ChangeFrequency.HOT,
    )

    assert awareness.file_path == "src/main.rs"
    assert awareness.modified_by == Source.AI_AGENT
    assert awareness.change_frequency == ChangeFrequency.HOT
    assert awareness.line_count == 100


def test_tree_node_file() -> None:
    node = TreeNode.file("main.rs", "src/main.rs")

    assert node.name == "main.rs"
    assert node.path == "src/main.rs"
    assert not node.is_dir
    assert node.children == []


def test_tree_node_dir() -> None:
    node = TreeNode.dir("src", "src")

    assert node.name == "src"
    assert node.path == "src"
    assert node.is_dir
    assert node.children == []


def test_tree_node_with_children() -> None:
    tree = TreeNode.dir("root", "")
    tree.children.extend([
        TreeNode.file("README.md", "README.md"),
        TreeNode.dir("src", "src"),
    ])

    assert len(tree.children) == 2
    assert tree.children[0].name == "README.md"
    assert tree.children[1].is_dir


def test_event_filter_default() -> None:
    f = EventFilter()

    assert f.project_id is None
    assert f.since is None
    assert f.source is None
    assert f.limit is None


def test_event_filter_with_values() -> None:
    f = EventFilter(
        project_id="proj-123",
        since=1708100000,
        source="ai_agent",
        limit=100,
    )

    assert f.project_id == "proj-123"
    assert f.since == 1708100000
    assert f.source == "ai_agent"
    assert f.limit == 100


def test_file_event_serialization() -> None:
    event = FileEvent(
        timestamp="2024-02-16T10:32:00Z",
        event_type=EventType.MODIFIED,
        file_path="src/lib.rs",
        project_id="proj-1",
        source=Source.GIT,
        source_id="abc123",
        machine_id="m1",
    )

    # Convert to dict (like json.dumps would)
    data = {
        "timestamp": event.timestamp,
        "event_type": event.event_type.value,
        "file_path": event.file_path,
        "project_id": event.project_id,
        "source": event.source.value,
        "source_id": event.source_id,
        "machine_id": event.machine_id,
        "content_hash": event.content_hash,
        "old_path": event.old_path,
    }

    # Round-trip through JSON
    json_str = json.dumps(data)
    parsed = json.loads(json_str)

    assert parsed["timestamp"] == "2024-02-16T10:32:00Z"
    assert parsed["event_type"] == "modified"
    assert parsed["file_path"] == "src/lib.rs"
    assert parsed["source"] == "git"
    assert parsed["source_id"] == "abc123"


def test_file_awareness_serialization() -> None:
    awareness = FileAwareness(
        file_path="src/lib.rs",
        project_id="proj-1",
        last_modified="2024-02-16T10:32:00Z",
        modified_by=Source.GIT,
        modified_by_label="Fix auth",
        todo_count=1,
        lint_hints=0,
        line_count=142,
        change_frequency=ChangeFrequency.WARM,
    )

    data = {
        "file_path": awareness.file_path,
        "project_id": awareness.project_id,
        "last_modified": awareness.last_modified,
        "modified_by": awareness.modified_by.value,
        "modified_by_label": awareness.modified_by_label,
        "active_agent": awareness.active_agent,
        "chat_references": awareness.chat_references,
        "todo_count": awareness.todo_count,
        "lint_hints": awareness.lint_hints,
        "line_count": awareness.line_count,
        "change_frequency": awareness.change_frequency.value,
    }

    json_str = json.dumps(data)
    parsed = json.loads(json_str)

    assert parsed["file_path"] == "src/lib.rs"
    assert parsed["modified_by"] == "git"
    assert parsed["change_frequency"] == "warm"
    assert parsed["line_count"] == 142


def test_tree_node_serialization() -> None:
    tree = TreeNode.dir("root", "")
    tree.children = [
        TreeNode.file("README.md", "README.md"),
        TreeNode.dir("src", "src"),
    ]

    data = {
        "name": tree.name,
        "path": tree.path,
        "is_dir": tree.is_dir,
        "children": [
            {
                "name": tree.children[0].name,
                "path": tree.children[0].path,
                "is_dir": tree.children[0].is_dir,
                "children": tree.children[0].children,
            },
            {
                "name": tree.children[1].name,
                "path": tree.children[1].path,
                "is_dir": tree.children[1].is_dir,
                "children": tree.children[1].children,
            },
        ],
    }

    json_str = json.dumps(data)
    parsed = json.loads(json_str)

    assert parsed["name"] == "root"
    assert parsed["is_dir"] is True
    assert len(parsed["children"]) == 2
    assert parsed["children"][0]["name"] == "README.md"
    assert parsed["children"][1]["is_dir"] is True


def test_notification_types() -> None:
    event = FileEvent(
        timestamp="2024-02-16T10:32:00Z",
        event_type=EventType.CREATED,
        file_path="test.py",
        project_id="proj-1",
        source=Source.USER,
        machine_id="m1",
    )

    notif_payload = FileEventNotification(event=event)
    notification = Notification(project_id="proj-1", payload=notif_payload)

    assert notification.project_id == "proj-1"
    assert isinstance(notification.payload, FileEventNotification)
    assert notification.payload.event.file_path == "test.py"
