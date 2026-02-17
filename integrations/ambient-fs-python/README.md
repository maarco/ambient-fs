# ambient-fs-python

Python client for the [ambient-fs](https://github.com/malmazan/ambient-fs) filesystem awareness daemon.

## Installation

```bash
pip install ambient-fs
```

## Quick Start

### Async Client

```python
import asyncio
from ambient_fs import AmbientFsClient, EventFilter

async def main():
    async with await AmbientFsClient.connect() as client:
        # Watch a project
        result = await client.watch_project("/home/user/project")
        print(f"Watching: {result.project_id}")

        # Query recent events
        events = await client.query_events(
            EventFilter(project_id=result.project_id, limit=10)
        )
        for event in events:
            print(f"{event.timestamp}: {event.event_type} {event.file_path}")

        # Get file awareness
        awareness = await client.query_awareness(result.project_id, "src/main.rs")
        print(f"{awareness.file_path}: {awareness.change_frequency}, {awareness.line_count} lines")

asyncio.run(main())
```

### Sync Client

```python
from ambient_fs import AmbientFsSyncClient, EventFilter

with AmbientFsSyncClient() as client:
    result = client.watch_project(".")
    events = client.query_events(
        EventFilter(project_id=result.project_id)
    )
    for event in events[:5]:
        print(f"{event.file_path}: {event.event_type}")
```

## Configuration

The client connects to the daemon via Unix socket. The socket path is resolved in this order:

1. Explicit `socket_path` parameter
2. `AMBIENT_FS_SOCK` environment variable
3. Default: `/tmp/ambient-fs.sock`

```python
# Explicit path
client = await AmbientFsClient.connect("/var/run/ambient-fs.sock")

# Or via environment variable
# export AMBIENT_FS_SOCK=/var/run/ambient-fs.sock
client = await AmbientFsClient.connect()
```

## API Reference

### AmbientFsClient (async)

- `await connect(socket_path=None)` - Connect to daemon
- `await watch_project(path)` - Watch a directory, returns `WatchResult`
- `await unwatch_project(project_id)` - Stop watching
- `await query_events(filter=None)` - Query event log
- `await query_awareness(project_id, path)` - Get file awareness
- `await query_tree(project_id)` - Get file tree
- `await attribute(project_id, file_path, source, source_id=None)` - Attribute a change
- `subscribe(project_id, callback)` - Subscribe to notifications
- `await close()` - Close connection

### Types

- `FileEvent` - Filesystem event with attribution
- `FileAwareness` - Aggregated file state
- `TreeNode` - File tree node
- `EventFilter` - Query filter
- `WatchResult` - Watch result
- `EventType` - Event type enum (CREATED, MODIFIED, DELETED, RENAMED)
- `Source` - Change source enum (USER, AI_AGENT, GIT, BUILD, VOICE)
- `ChangeFrequency` - Frequency enum (HOT, WARM, COLD)

### Errors

- `AmbientFsError` - Base exception
- `ConnectionError` - Failed to connect
- `DaemonError` - Daemon returned an error (has `code` and `message` attributes)
- `TimeoutError` - Request timed out
- `InvalidResponseError` - Invalid daemon response

## Development

```bash
# Install dev dependencies
pip install -e ".[dev]"

# Run tests
pytest -v

# Type check
mypy src/
```

## License

MIT
