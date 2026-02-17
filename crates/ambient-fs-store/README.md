# ambient-fs-store

SQLite event store for ambient-fs. Append-only event log with WAL mode for concurrent reads.

## Features

- **EventStore** - Insert and query file events
  - `insert(event)` - Write single event
  - `insert_batch(events)` - Write multiple events
  - `query(filter)` - Query with filters (project, source, since, limit)
  - `get_latest(project, path)` - Most recent event for a file

- **FileAnalysisCache** - Cache analysis results
  - `get(project, path)` - Retrieve cached analysis
  - `put(analysis)` - Store analysis result
  - `invalidate(project, path)` - Remove from cache

- **EventPruner** - Retention policy
  - `prune_events_before(cutoff)` - Delete old events
  - `vacuum(conn)` - Reclaim space

- **Migrations** - Schema versioning
  - `ensure_schema(conn)` - Run pending migrations

## Schema

```sql
CREATE TABLE file_events (
    id INTEGER PRIMARY KEY,
    timestamp TEXT NOT NULL,
    event_type TEXT NOT NULL,
    file_path TEXT NOT NULL,
    project_id TEXT NOT NULL,
    source TEXT NOT NULL,
    source_id TEXT,
    machine_id TEXT NOT NULL,
    content_hash TEXT
);

CREATE TABLE file_analysis (
    file_path TEXT NOT NULL,
    project_id TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    analyzed_at INTEGER NOT NULL,
    exports TEXT,
    imports TEXT,
    todo_count INTEGER,
    line_count INTEGER,
    PRIMARY KEY (project_id, file_path)
);
```

## Usage

```rust
use ambient_fs_store::EventStore;

let store = EventStore::new("/path/to/events.db")?;

store.insert(&event)?;
let events = store.query(EventFilter::new().project_id("my-project"))?;
```

## License

MIT
